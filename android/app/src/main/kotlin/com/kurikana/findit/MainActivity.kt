package com.kurikana.findit

import android.os.Bundle
import android.view.WindowManager
import io.flutter.embedding.android.FlutterActivity

class MainActivity : FlutterActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // S-I1：防截屏/防最近任务缩略图泄露物品清单与照片。
        // 说明：本开关会同时禁用录屏与截图分享；应用锁（生物识别/PIN）列为后续项，
        // 本次仅做防截屏加固，不引入额外交互。
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
    }
}
