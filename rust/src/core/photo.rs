//! 物品照片：压缩管道与文件管理。
//!
//! 压缩管道为纯函数（输入字节 → 主图/缩略图字节），不触碰数据库；
//! `save_item_photo` / `delete_item_photo` 等编排函数接收 `&Connection`，
//! 负责「压缩落盘 + 更新 `items.photo_path` + 清理旧文件」的事务性流程。
//!
//! 文件布局（`photos_dir` 即 `db_dir/photos`，由 [`crate::core::db::init_db`] 创建）：
//! - 主图：`{uuid}.jpg`，最长边 1600px，JPEG q82
//! - 缩略图：`{uuid}_thumb.jpg`，最长边 256px，JPEG q70

use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, ImageReader};
use rusqlite::Connection;
use uuid::Uuid;

use crate::core::db;
use crate::core::error::{FinditError, FinditResult};
use crate::core::repo;

/// 主图最长边上限（像素）。
pub const MAIN_MAX_EDGE: u32 = 1600;
/// 缩略图最长边上限（像素）。
pub const THUMB_MAX_EDGE: u32 = 256;
/// 主图 JPEG 质量。
pub const MAIN_QUALITY: u8 = 82;
/// 缩略图 JPEG 质量。
pub const THUMB_QUALITY: u8 = 70;

/// 主图文件名 → 缩略图文件名（`abc.jpg` → `abc_thumb.jpg`）。
pub fn thumb_file_name(main_file_name: &str) -> String {
    match main_file_name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}_thumb.{ext}"),
        None => format!("{main_file_name}_thumb"),
    }
}

/// 校验照片文件名：必须形如 `xxx.jpg`，禁止路径分隔符与目录穿越。
fn validate_file_name(name: &str) -> FinditResult<()> {
    let ok = !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name != "."
        && name != "..";
    if !ok {
        return Err(FinditError::Validation(format!("非法的照片文件名：{name}")));
    }
    Ok(())
}

/// 按比例缩小至最长边不超过 `max_edge`；小于该尺寸时不放大、原样返回。
fn resize_to_fit(img: &DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest <= max_edge {
        return img.clone();
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    let new_w = ((f64::from(w) * scale).round() as u32).max(1);
    let new_h = ((f64::from(h) * scale).round() as u32).max(1);
    img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3)
}

/// 编码为指定质量的 JPEG 字节。
fn encode_jpeg(img: &DynamicImage, quality: u8) -> FinditResult<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, quality);
    encoder
        .encode_image(img)
        .map_err(|e| FinditError::Io(format!("JPEG 编码失败：{e}")))?;
    Ok(buf)
}

/// 写入一个 JPEG 文件。
fn write_jpeg(path: &Path, img: &DynamicImage, quality: u8) -> FinditResult<()> {
    let bytes = encode_jpeg(img, quality)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// 解码尺寸上限（最长边，像素）：超过该上限的图片直接拒绝，
/// 防止异常超大图把内存打爆（P-M7 的兜底防线）。
pub const MAX_DECODE_EDGE: u32 = 16_384;

/// 受限解码：JPEG 走 scale-down 快速降采样（1/2、1/4、1/8，P-M7），
/// 避免全分辨率解码的内存与 CPU 峰值；其它格式先读头部尺寸，超限拒绝。
fn decode_limited(raw: &[u8]) -> FinditResult<DynamicImage> {
    let format = image::guess_format(raw)
        .map_err(|e| FinditError::Validation(format!("无法识别图片格式：{e}")))?;

    if format == image::ImageFormat::Jpeg {
        if let Some(img) = try_decode_jpeg_scaled(raw)? {
            return Ok(img);
        }
        // CMYK 等非常见输出格式：走下方全量解码兜底。
    }

    // 非 JPEG（或 JPEG 兜底）：先读头部尺寸，超限拒绝，避免全量解码。
    let dims = ImageReader::new(std::io::Cursor::new(raw))
        .with_guessed_format()
        .map_err(|e| FinditError::Validation(format!("无法解析图片：{e}")))?
        .into_dimensions()
        .map_err(|e| FinditError::Validation(format!("无法解析图片尺寸：{e}")))?;
    if dims.0.max(dims.1) > MAX_DECODE_EDGE {
        return Err(FinditError::Validation(format!(
            "照片尺寸过大（{0}×{1}，上限 {MAX_DECODE_EDGE} 像素）",
            dims.0, dims.1
        )));
    }
    image::load_from_memory(raw)
        .map_err(|e| FinditError::Validation(format!("无法解析图片：{e}")))
}

/// JPEG 降采样解码：按最长边选择 1/2、1/4、1/8 缩放因子，
/// 解码结果尺寸 ≈ 最长边 ≥ `MAIN_MAX_EDGE` 的最小档位。
/// 返回 `None` 表示输出格式非 RGB/Luma（调用方全量解码兜底）。
fn try_decode_jpeg_scaled(raw: &[u8]) -> FinditResult<Option<DynamicImage>> {
    use jpeg_decoder::PixelFormat;

    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(raw));
    decoder
        .read_info()
        .map_err(|e| FinditError::Validation(format!("无法解析 JPEG：{e}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| FinditError::Validation("JPEG 缺少头部信息".to_string()))?;
    let (w, h) = (info.width as u32, info.height as u32);
    if w.max(h) > MAX_DECODE_EDGE {
        return Err(FinditError::Validation(format!(
            "照片尺寸过大（{w}×{h}，上限 {MAX_DECODE_EDGE} 像素）"
        )));
    }

    // 目标：降采样后最长边仍 ≥ 主图上限，避免二次放大。
    // jpeg-decoder 0.3 的 `scale(requested_w, requested_h)` 自动选择
    // 1/8、1/4、1/2、1 中「输出在任一轴 ≥ 请求尺寸」的最大降采样档位
    // （内部 `choose_idct_size` 从最激进档位开始尝试），与旧 `set_scale`
    // 循环的语义一致；请求尺寸取主图上限即可保证降采样后无需再放大。
    if w.max(h) > MAIN_MAX_EDGE {
        decoder
            .scale(MAIN_MAX_EDGE as u16, MAIN_MAX_EDGE as u16)
            .map_err(|e| FinditError::Validation(format!("JPEG 降采样配置失败：{e}")))?;
    }

    let pixels = decoder
        .decode()
        .map_err(|e| FinditError::Validation(format!("JPEG 解码失败：{e}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| FinditError::Validation("JPEG 解码后缺少尺寸信息".to_string()))?;
    let (dw, dh) = (info.width as u32, info.height as u32);

    match info.pixel_format {
        PixelFormat::RGB24 => Ok(Some(DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(dw, dh, pixels)
                .ok_or_else(|| FinditError::Io("JPEG 像素缓冲尺寸不一致".to_string()))?,
        ))),
        PixelFormat::L8 => Ok(Some(DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(dw, dh, pixels)
                .ok_or_else(|| FinditError::Io("JPEG 像素缓冲尺寸不一致".to_string()))?,
        ))),
        _ => Ok(None),
    }
}

/// 压缩管道（纯函数，不碰数据库）：
/// 解码原始图片字节 → 主图（最长边 1600，q82）+ 缩略图（最长边 256，q70）
/// → 写入 `photos_dir`，返回主图文件名 `{uuid}.jpg`。
///
/// P-M7：JPEG 输入走 scale-down 降采样解码，大图内存/CPU 峰值可控。
pub fn save_photo_bytes(raw: &[u8], photos_dir: &Path) -> FinditResult<String> {
    if raw.is_empty() {
        return Err(FinditError::Validation("照片数据为空".to_string()));
    }
    let img = decode_limited(raw)?;

    let stem = Uuid::new_v4().to_string();
    let main_name = format!("{stem}.jpg");

    let main_img = resize_to_fit(&img, MAIN_MAX_EDGE);
    let thumb_img = resize_to_fit(&img, THUMB_MAX_EDGE);

    std::fs::create_dir_all(photos_dir)?;
    write_jpeg(&photos_dir.join(&main_name), &main_img, MAIN_QUALITY)?;
    write_jpeg(
        &photos_dir.join(thumb_file_name(&main_name)),
        &thumb_img,
        THUMB_QUALITY,
    )?;

    Ok(main_name)
}

/// 删除照片对应的两个文件（主图 + 缩略图）；文件不存在时静默忽略。
pub fn delete_photo_files(photos_dir: &Path, main_file_name: &str) -> FinditResult<()> {
    validate_file_name(main_file_name)?;
    for name in [main_file_name.to_string(), thumb_file_name(main_file_name)] {
        let path = photos_dir.join(&name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// 尽力清理一批 `photo_path` 对应的主图与缩略图文件（供级联删除后调用）。
/// 任何失败仅记录日志不阻断：数据库事务已提交，残留文件不影响数据一致性。
pub fn cleanup_photo_paths_best_effort(photo_paths: &[String]) {
    let dir = match db::photos_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    for path in photo_paths {
        if let Err(e) = delete_photo_files(&dir, path) {
            eprintln!("[findit] 清理照片文件失败 {path}：{e}");
        }
    }
}

/// 主图完整路径（供 Dart 侧用 `File` 直接加载）。
pub fn photo_full_path(file_name: &str) -> FinditResult<String> {
    validate_file_name(file_name)?;
    let dir = db::photos_dir()?;
    Ok(dir.join(file_name).to_string_lossy().into_owned())
}

/// 缩略图完整路径（入参为主图文件名）。
pub fn thumb_full_path(file_name: &str) -> FinditResult<String> {
    photo_full_path(&thumb_file_name(file_name))
}

/// P-H1 三阶段保存的「阶段 1」（锁内，极短）：校验物品存在并取旧照片路径。
/// 图片压缩不在此阶段执行，锁内只做一次主键查询。
pub fn existing_item_photo(conn: &Connection, item_id: i64) -> FinditResult<Option<String>> {
    Ok(repo::items::get_item(conn, item_id)?.photo_path)
}

/// P-H1 三阶段保存的「阶段 3」（锁内，极短）：登记新照片路径并清理旧文件。
/// 压缩落盘已在锁外完成，这里只做一次 UPDATE + 旧文件清理。
pub fn register_item_photo(
    conn: &Connection,
    item_id: i64,
    new_name: &str,
    old_photo: Option<&str>,
) -> FinditResult<()> {
    repo::items::set_item_photo_path(conn, item_id, Some(new_name))?;
    // 旧文件清理失败不影响本次保存结果。
    if let Some(old_name) = old_photo {
        let _ = delete_photo_files(&db::photos_dir()?, old_name);
    }
    Ok(())
}

/// 保存物品照片（组合入口，供测试与不关心锁边界的调用方使用）：
/// 校验 → 压缩落盘 → 登记路径 → 清理旧文件。
///
/// 生产路径应改用三阶段编排（见 `crate::api::photos::save_item_photo`）：
/// 「锁内取旧路径 → 锁外压缩 → 锁内登记」，避免压缩期间持有全局数据库锁。
pub fn save_item_photo(conn: &Connection, item_id: i64, raw: &[u8]) -> FinditResult<String> {
    let old_photo = existing_item_photo(conn, item_id)?;
    let new_name = save_photo_bytes(raw, &db::photos_dir()?)?;
    register_item_photo(conn, item_id, &new_name, old_photo.as_deref())?;
    Ok(new_name)
}

/// 删除物品照片：清空 `photo_path` 并删除主图与缩略图文件。
/// 物品本无照片时静默成功。
pub fn delete_item_photo(conn: &Connection, item_id: i64) -> FinditResult<()> {
    let photos_dir = db::photos_dir()?;
    let item = repo::items::get_item(conn, item_id)?;
    if let Some(name) = item.photo_path {
        repo::items::set_item_photo_path(conn, item_id, None)?;
        delete_photo_files(&photos_dir, &name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;
    use crate::core::repo::{boxes, units};
    use image::ImageEncoder;
    use image::RgbaImage;

    /// 生成 w×h 的渐变测试图。
    fn test_image(w: u32, h: u32) -> DynamicImage {
        let mut img = RgbaImage::new(w, h);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = image::Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                ((x + y) % 256) as u8,
                255,
            ]);
        }
        DynamicImage::ImageRgba8(img)
    }

    /// 把测试图编码为 JPEG 字节，模拟相机/相册原始输入。
    fn test_jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
        encode_jpeg(&test_image(w, h), 95).unwrap()
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "findit-photo-test-{tag}-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 打开目录内的照片文件并返回其尺寸（同时验证其为合法图片）。
    fn dimensions(dir: &Path, name: &str) -> (u32, u32) {
        let img = image::open(dir.join(name)).unwrap();
        img.dimensions()
    }

    #[test]
    fn thumb_file_name_mapping() {
        assert_eq!(thumb_file_name("abc.jpg"), "abc_thumb.jpg");
        assert_eq!(thumb_file_name("no-ext"), "no-ext_thumb");
    }

    #[test]
    fn save_photo_bytes_downsizes_large_image() {
        let dir = temp_dir("downsize");
        let name = save_photo_bytes(&test_jpeg_bytes(3200, 2400), &dir).unwrap();

        assert!(name.ends_with(".jpg"));
        assert!(dir.join(&name).exists(), "主图文件应写入");
        let thumb = thumb_file_name(&name);
        assert!(dir.join(&thumb).exists(), "缩略图文件应写入");

        let (mw, mh) = dimensions(&dir, &name);
        assert_eq!((mw, mh), (1600, 1200), "主图最长边应为 1600");
        let (tw, th) = dimensions(&dir, &thumb);
        assert_eq!((tw, th), (256, 192), "缩略图最长边应为 256");
    }

    #[test]
    fn save_photo_bytes_keeps_small_image_size() {
        let dir = temp_dir("small");
        let name = save_photo_bytes(&test_jpeg_bytes(120, 80), &dir).unwrap();
        assert_eq!(dimensions(&dir, &name), (120, 80), "小图不应放大");
        assert_eq!(
            dimensions(&dir, &thumb_file_name(&name)),
            (120, 80),
            "缩略图不应放大"
        );
    }

    #[test]
    fn save_photo_bytes_portrait_orientation() {
        let dir = temp_dir("portrait");
        let name = save_photo_bytes(&test_jpeg_bytes(900, 2000), &dir).unwrap();
        assert_eq!(dimensions(&dir, &name), (720, 1600));
    }

    #[test]
    fn jpeg_scale_down_decode_reduces_resolution() {
        // P-M7 回归：大 JPEG 应降采样解码（1/2），而非全分辨率解码。
        let img = try_decode_jpeg_scaled(&test_jpeg_bytes(3200, 2400))
            .unwrap()
            .unwrap();
        assert_eq!(img.dimensions(), (1600, 1200), "3200px 长边应降采样到 1/2");
    }

    #[test]
    fn jpeg_scale_down_skipped_for_small_images() {
        // 小图（≤ 主图上限）不应降采样，避免画质损失。
        let img = try_decode_jpeg_scaled(&test_jpeg_bytes(900, 600))
            .unwrap()
            .unwrap();
        assert_eq!(img.dimensions(), (900, 600));
    }

    #[test]
    fn decode_limited_rejects_oversized_image() {
        // P-M7 兜底：超上限尺寸（> 16384 长边）直接拒绝，避免内存打爆。
        let big = image::RgbaImage::from_pixel(17_000, 10, image::Rgba([0u8, 0, 0, 255]));
        let mut buf = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(
                big.as_raw(),
                17_000,
                10,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let dir = temp_dir("oversized");
        assert!(matches!(
            save_photo_bytes(&buf, &dir),
            Err(FinditError::Validation(_))
        ));
    }

    #[test]
    fn save_photo_bytes_rejects_invalid_input() {
        let dir = temp_dir("invalid");
        assert!(matches!(
            save_photo_bytes(&[], &dir),
            Err(FinditError::Validation(_))
        ));
        assert!(matches!(
            save_photo_bytes(b"not an image at all", &dir),
            Err(FinditError::Validation(_))
        ));
    }

    #[test]
    fn delete_photo_files_removes_both_and_ignores_missing() {
        let dir = temp_dir("delete");
        let name = save_photo_bytes(&test_jpeg_bytes(300, 200), &dir).unwrap();
        let thumb = thumb_file_name(&name);
        assert!(dir.join(&name).exists() && dir.join(&thumb).exists());

        delete_photo_files(&dir, &name).unwrap();
        assert!(!dir.join(&name).exists(), "主图应被删除");
        assert!(!dir.join(&thumb).exists(), "缩略图应被删除");

        // 重复删除（文件已不存在）应静默成功。
        delete_photo_files(&dir, &name).unwrap();
    }

    #[test]
    fn validate_file_name_rejects_traversal() {
        assert!(validate_file_name("abc.jpg").is_ok());
        assert!(validate_file_name("../etc/passwd").is_err());
        assert!(validate_file_name("a/b.jpg").is_err());
        assert!(validate_file_name("a\\b.jpg").is_err());
        assert!(validate_file_name("").is_err());
    }

    /// 端到端：照片目录 + 数据库联动（涉及全局状态，合并为单个用例）。
    #[test]
    fn item_photo_lifecycle() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let unit = units::create_unit(&conn, "柜子", "").unwrap();
        let box_ = boxes::create_box(&conn, unit.id, "箱子", "").unwrap();
        let item = repo::items::create_item(&conn, box_.id, "相机", "", 1, &[]).unwrap();

        let dir = temp_dir("lifecycle");
        db::set_photos_dir_for_test(dir.clone());

        // 首次保存
        let name1 = save_item_photo(&conn, item.id, &test_jpeg_bytes(400, 300)).unwrap();
        let reloaded = repo::items::get_item(&conn, item.id).unwrap();
        assert_eq!(reloaded.photo_path.as_deref(), Some(name1.as_str()));
        assert!(dir.join(&name1).exists());
        assert!(dir.join(thumb_file_name(&name1)).exists());

        // 路径辅助
        assert!(photo_full_path(&name1).unwrap().ends_with(&name1));
        assert!(thumb_full_path(&name1)
            .unwrap()
            .ends_with(&thumb_file_name(&name1)));

        // 替换照片：旧文件被清理
        let name2 = save_item_photo(&conn, item.id, &test_jpeg_bytes(500, 400)).unwrap();
        assert_ne!(name1, name2);
        assert!(!dir.join(&name1).exists(), "旧主图应被清理");
        assert!(
            !dir.join(thumb_file_name(&name1)).exists(),
            "旧缩略图应被清理"
        );
        assert!(dir.join(&name2).exists());

        // 删除照片：字段清空 + 文件移除
        delete_item_photo(&conn, item.id).unwrap();
        let reloaded = repo::items::get_item(&conn, item.id).unwrap();
        assert!(reloaded.photo_path.is_none());
        assert!(!dir.join(&name2).exists());
        assert!(!dir.join(thumb_file_name(&name2)).exists());

        // 无照片时再次删除静默成功
        delete_item_photo(&conn, item.id).unwrap();

        // 不存在的物品报错
        assert!(matches!(
            save_item_photo(&conn, 999, &test_jpeg_bytes(100, 100)),
            Err(FinditError::NotFound { .. })
        ));
        assert!(matches!(
            delete_item_photo(&conn, 999),
            Err(FinditError::NotFound { .. })
        ));

        // 还原全局状态，避免影响其它测试
        db::set_photos_dir_for_test(std::env::temp_dir());
    }
}
