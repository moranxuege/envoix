plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jlleitschuh.gradle.ktlint")
}

// Short git SHA embedded into BuildConfig, so any uploaded log identifies exactly
// which build produced it. Falls back to "unknown" outside a git checkout.
val gitCommit: String =
    try {
        ProcessBuilder("git", "rev-parse", "--short", "HEAD")
            .directory(rootDir)
            .redirectErrorStream(true)
            .start()
            .inputStream
            .bufferedReader()
            .readText()
            .trim()
            .ifEmpty { "unknown" }
    } catch (e: Exception) {
        "unknown"
    }

android {
    namespace = "dev.envoix.app"
    compileSdk = 34

    defaultConfig {
        applicationId = "dev.envoix.app"
        minSdk = 29 // Android 10: scoped storage + MediaStore.Downloads
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
        buildConfigField("String", "GIT_COMMIT", "\"$gitCommit\"")
        ndk {
            // Ship the envoix binary only for the ABIs we cross-compile.
            abiFilters += listOf("x86_64", "arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Debug-signed so the shrunk APK stays installable for testers.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    buildFeatures {
        compose = true
        buildConfig = true // for BuildConfig.GIT_COMMIT / VERSION_NAME
    }

    // The envoix CLI ships as libenvoix.so in jniLibs; legacy packaging extracts
    // it to the app's native-lib dir, the one place Android lets us exec a file.
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }
}

ktlint {
    android = true
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.09.02")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.documentfile:documentfile:1.0.1") // SAF save-folder picker
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.6")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.6")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.6")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    implementation("com.google.zxing:core:3.5.3") // QR encode + decode
    // CameraX for the custom QR scanner (preview + frame analysis)
    implementation("androidx.camera:camera-core:1.3.4")
    implementation("androidx.camera:camera-camera2:1.3.4")
    implementation("androidx.camera:camera-lifecycle:1.3.4")
    implementation("androidx.camera:camera-view:1.3.4")
}
