import 'package:flutter/material.dart';

/// Findit 视觉基调：暖纸底色 + 松木绿主色 + 柿子橙点缀，
/// 走「收纳标签 / 档案卡片」风格，避免通用的紫色渐变。
class FinditColors {
  FinditColors._();

  static const Color paper = Color(0xFFF6F1E6);
  static const Color ink = Color(0xFF292418);
  static const Color inkSoft = Color(0xFF7A7160);
  static const Color pine = Color(0xFF2F5D50);
  static const Color pineDeep = Color(0xFF24473D);
  static const Color persimmon = Color(0xFFD96C3F);
  static const Color cardLine = Color(0xFFE4DCC8);
  static const Color cardFace = Color(0xFFFFFDF7);
  static const Color chipFill = Color(0xFFEDE6D3);
  static const Color danger = Color(0xFFB3402F);
}

ThemeData get finditTheme {
  const ink = FinditColors.ink;
  final base = ThemeData(
    useMaterial3: true,
    colorScheme: ColorScheme.fromSeed(
      seedColor: FinditColors.pine,
      brightness: Brightness.light,
      surface: FinditColors.cardFace,
    ).copyWith(
      primary: FinditColors.pine,
      secondary: FinditColors.persimmon,
      error: FinditColors.danger,
      onPrimary: Colors.white,
    ),
    scaffoldBackgroundColor: FinditColors.paper,
  );

  final textTheme = base.textTheme.apply(
    bodyColor: ink,
    displayColor: ink,
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
    appBarTheme: const AppBarTheme(
      backgroundColor: FinditColors.paper,
      foregroundColor: ink,
      elevation: 0,
      scrolledUnderElevation: 0,
      centerTitle: false,
      titleTextStyle: TextStyle(
        color: ink,
        fontSize: 22,
        fontWeight: FontWeight.w800,
        letterSpacing: -0.3,
      ),
    ),
    cardTheme: CardThemeData(
      color: FinditColors.cardFace,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(14),
        side: const BorderSide(color: FinditColors.cardLine),
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: Colors.white,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(12),
        borderSide: const BorderSide(color: FinditColors.cardLine),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(12),
        borderSide: const BorderSide(color: FinditColors.cardLine),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(12),
        borderSide: const BorderSide(color: FinditColors.pine, width: 2),
      ),
      contentPadding:
          const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: FinditColors.cardFace,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(18),
        side: const BorderSide(color: FinditColors.cardLine),
      ),
    ),
    floatingActionButtonTheme: const FloatingActionButtonThemeData(
      backgroundColor: FinditColors.pine,
      foregroundColor: Colors.white,
    ),
    dividerTheme: const DividerThemeData(
      color: FinditColors.cardLine,
      thickness: 1,
    ),
  );
}
