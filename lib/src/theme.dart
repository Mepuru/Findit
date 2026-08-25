import 'package:flutter/material.dart';

/// Findit 视觉基调：暖纸底色 + 松木绿主色 + 柿子橙点缀，
/// 走「收纳标签 / 档案卡片」风格，避免通用的紫色渐变。
///
/// 双主题：亮色沿用暖纸档案感；暗色换成深褐木柜底色，
/// 松木绿 / 柿子橙按暗底可读性提亮。
class FinditPalette extends ThemeExtension<FinditPalette> {
  const FinditPalette({
    required this.paper,
    required this.ink,
    required this.inkSoft,
    required this.pine,
    required this.pineDeep,
    required this.persimmon,
    required this.cardLine,
    required this.cardFace,
    required this.chipFill,
    required this.danger,
  });

  /// 页面底色（亮：暖纸；暗：深褐）。
  final Color paper;

  /// 主文字色。
  final Color ink;

  /// 次级文字色。
  final Color inkSoft;

  /// 主色：松木绿。
  final Color pine;

  /// 松木绿加深（标签文字 / 分区标题）。
  final Color pineDeep;

  /// 点缀色：柿子橙。
  final Color persimmon;

  /// 卡片描边 / 分隔线。
  final Color cardLine;

  /// 卡片底色。
  final Color cardFace;

  /// 徽章 / 头像底色。
  final Color chipFill;

  /// 危险操作色。
  final Color danger;

  static const FinditPalette light = FinditPalette(
    paper: Color(0xFFF6F1E6),
    ink: Color(0xFF292418),
    inkSoft: Color(0xFF7A7160),
    pine: Color(0xFF2F5D50),
    pineDeep: Color(0xFF24473D),
    persimmon: Color(0xFFD96C3F),
    cardLine: Color(0xFFE4DCC8),
    cardFace: Color(0xFFFFFDF7),
    chipFill: Color(0xFFEDE6D3),
    danger: Color(0xFFB3402F),
  );

  static const FinditPalette dark = FinditPalette(
    paper: Color(0xFF151310),
    ink: Color(0xFFEFE7D6),
    inkSoft: Color(0xFFA39A86),
    pine: Color(0xFF63A08D),
    pineDeep: Color(0xFF8CC4B0),
    persimmon: Color(0xFFE58A5D),
    cardLine: Color(0xFF37322A),
    cardFace: Color(0xFF1F1C16),
    chipFill: Color(0xFF2C2820),
    danger: Color(0xFFE0705C),
  );

  @override
  FinditPalette copyWith({
    Color? paper,
    Color? ink,
    Color? inkSoft,
    Color? pine,
    Color? pineDeep,
    Color? persimmon,
    Color? cardLine,
    Color? cardFace,
    Color? chipFill,
    Color? danger,
  }) {
    return FinditPalette(
      paper: paper ?? this.paper,
      ink: ink ?? this.ink,
      inkSoft: inkSoft ?? this.inkSoft,
      pine: pine ?? this.pine,
      pineDeep: pineDeep ?? this.pineDeep,
      persimmon: persimmon ?? this.persimmon,
      cardLine: cardLine ?? this.cardLine,
      cardFace: cardFace ?? this.cardFace,
      chipFill: chipFill ?? this.chipFill,
      danger: danger ?? this.danger,
    );
  }

  @override
  FinditPalette lerp(FinditPalette? other, double t) {
    if (other == null) return this;
    return FinditPalette(
      paper: Color.lerp(paper, other.paper, t)!,
      ink: Color.lerp(ink, other.ink, t)!,
      inkSoft: Color.lerp(inkSoft, other.inkSoft, t)!,
      pine: Color.lerp(pine, other.pine, t)!,
      pineDeep: Color.lerp(pineDeep, other.pineDeep, t)!,
      persimmon: Color.lerp(persimmon, other.persimmon, t)!,
      cardLine: Color.lerp(cardLine, other.cardLine, t)!,
      cardFace: Color.lerp(cardFace, other.cardFace, t)!,
      chipFill: Color.lerp(chipFill, other.chipFill, t)!,
      danger: Color.lerp(danger, other.danger, t)!,
    );
  }
}

/// 便捷访问：`context.palette` 取当前亮/暗调色板。
extension FinditThemeX on BuildContext {
  FinditPalette get palette => Theme.of(this).extension<FinditPalette>()!;
}

/// 亮色主题（跟随系统，另见 [finditDarkTheme]）。
ThemeData get finditTheme => _buildTheme(Brightness.light, FinditPalette.light);

/// 暗色主题。
ThemeData get finditDarkTheme => _buildTheme(Brightness.dark, FinditPalette.dark);

ThemeData _buildTheme(Brightness brightness, FinditPalette p) {
  final base = ThemeData(
    useMaterial3: true,
    brightness: brightness,
    colorScheme: ColorScheme.fromSeed(
      seedColor: p.pine,
      brightness: brightness,
      surface: p.cardFace,
    ).copyWith(
      primary: p.pine,
      onPrimary: brightness == Brightness.light
          ? Colors.white
          : const Color(0xFF0F1E19),
      secondary: p.persimmon,
      onSecondary: brightness == Brightness.light
          ? Colors.white
          : const Color(0xFF2A1206),
      error: p.danger,
      onError: brightness == Brightness.light
          ? Colors.white
          : const Color(0xFF2A0C07),
    ),
    scaffoldBackgroundColor: p.paper,
    extensions: [p],
  );

  final textTheme = base.textTheme.apply(
    bodyColor: p.ink,
    displayColor: p.ink,
  );

  return base.copyWith(
    textTheme: textTheme.copyWith(
      titleLarge: textTheme.titleLarge!.copyWith(
        fontWeight: FontWeight.w800,
        letterSpacing: -0.3,
      ),
      titleMedium: textTheme.titleMedium!.copyWith(
        fontWeight: FontWeight.w700,
      ),
      labelSmall: textTheme.labelSmall!.copyWith(
        letterSpacing: 1.2,
        fontWeight: FontWeight.w600,
      ),
    ),
    appBarTheme: AppBarTheme(
      backgroundColor: p.paper,
      foregroundColor: p.ink,
      elevation: 0,
      scrolledUnderElevation: 0,
      centerTitle: false,
      titleTextStyle: TextStyle(
        color: p.ink,
        fontSize: 22,
        fontWeight: FontWeight.w800,
        letterSpacing: -0.3,
      ),
    ),
    cardTheme: CardThemeData(
      color: p.cardFace,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(14),
        side: BorderSide(color: p.cardLine),
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: p.cardFace,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(12),
        borderSide: BorderSide(color: p.cardLine),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(12),
        borderSide: BorderSide(color: p.cardLine),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(12),
        borderSide: BorderSide(color: p.pine, width: 2),
      ),
      contentPadding:
          const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: p.cardFace,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(18),
        side: BorderSide(color: p.cardLine),
      ),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: p.cardFace,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(22)),
        side: BorderSide(color: p.cardLine),
      ),
    ),
    floatingActionButtonTheme: FloatingActionButtonThemeData(
      backgroundColor: p.pine,
      foregroundColor: brightness == Brightness.light
          ? Colors.white
          : const Color(0xFF0F1E19),
    ),
    dividerTheme: DividerThemeData(
      color: p.cardLine,
      thickness: 1,
    ),
    snackBarTheme: SnackBarThemeData(
      backgroundColor: p.ink,
      contentTextStyle: TextStyle(color: p.paper, fontSize: 13),
      behavior: SnackBarBehavior.floating,
    ),
  );
}
