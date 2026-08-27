import java.io.FileInputStream
import java.util.Properties

plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

// Release signing (S-H1): load android/key.properties (gitignored, never committed).
// When the file is missing, RELEASE builds FAIL instead of silently falling back to
// the debug key -- the debug keystore is public knowledge and must never be shipped.
val keystoreProperties = Properties()
val keystorePropertiesFile = rootProject.file("key.properties")
val keystoreConfigured = keystorePropertiesFile.exists()
if (keystoreConfigured) {
    keystoreProperties.load(FileInputStream(keystorePropertiesFile))
}

android {
    namespace = "com.example.findit"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // 唯一应用包名（与 namespace 解耦，仅影响新安装）。
        applicationId = "com.kurikana.findit"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        // You can force using the value of versionCode by specifying the `-P force-version-code-ignoring-abi=true`
        // flag during build.
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs {
        create("release") {
            if (keystoreConfigured) {
                keyAlias = keystoreProperties["keyAlias"] as String
                keyPassword = keystoreProperties["keyPassword"] as String
                storeFile = file(keystoreProperties["storeFile"] as String)
                storePassword = keystoreProperties["storePassword"] as String
            }
        }
    }

    buildTypes {
        release {
            if (keystoreConfigured) {
                signingConfig = signingConfigs.getByName("release")
            } else {
                // Never fall back to the debug signature for release builds:
                // fail fast so a release APK can only be produced with a real key.
                val releaseRequested = gradle.startParameter.taskNames.any { task ->
                    task.lowercase().contains("release")
                }
                if (releaseRequested) {
                    throw GradleException(
                        "Missing android/key.properties: a release build requires an " +
                            "independent signing keystore (debug signing is insecure and " +
                            "must not be shipped). Create a release keystore and add " +
                            "key.properties with storeFile/storePassword/keyAlias/keyPassword. " +
                            "The file is gitignored and must never be committed."
                    )
                }
                signingConfig = signingConfigs.getByName("debug")
            }
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}
