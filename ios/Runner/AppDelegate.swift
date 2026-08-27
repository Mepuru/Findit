import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    // S-M3：隐私优先 —— 数据目录（含数据库与照片）不参与 iCloud 备份。
    excludeDocumentsFromBackup()
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  /// 把 Documents 目录标记为不参与 iCloud 备份（NSURLIsExcludedFromBackupKey）。
  /// 数据仍保存在应用沙盒内，仅避免自动上云。
  private func excludeDocumentsFromBackup() {
    guard
      let documentsURL = FileManager.default.urls(
        for: .documentDirectory, in: .userDomainMask
      ).first
    else { return }
    var url = documentsURL
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    do {
      try url.setResourceValues(values)
    } catch {
      // 失败不阻断启动；数据仍在本地沙盒内。
      NSLog("Findit: 无法排除文档目录的 iCloud 备份：\(error)")
    }
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
  }
}
