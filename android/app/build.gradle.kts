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

val envoixAndroidAbis =
    (
        providers.gradleProperty("envoix.android.abis").orNull
            ?: providers.environmentVariable("ENVOIX_ANDROID_ABIS").orNull
            ?: "arm64-v8a"
    ).split(",")
        .map { it.trim() }
        .filter { it.isNotEmpty() }

val envoixRustTargets =
    mapOf(
        "arm64-v8a" to "aarch64-linux-android",
        "x86_64" to "x86_64-linux-android",
    )

val envoixAndroidApiLevel = 26
val generatedJniLibsDir = layout.buildDirectory.dir("generated/envoix/jniLibs")

val buildEnvoixJniAndroid by tasks.registering {
    group = "build"
    description = "Builds and stages the hand-written Android JNI core."

    inputs.files(
        rootProject.layout.projectDirectory
            .dir("../apps/envoix-android-jni")
            .asFileTree,
        rootProject.layout.projectDirectory
            .dir("../crates")
            .asFileTree,
        rootProject.layout.projectDirectory.file("../Cargo.toml"),
        rootProject.layout.projectDirectory.file("../Cargo.lock"),
    )
    inputs.property("androidAbis", envoixAndroidAbis)
    outputs.dir(generatedJniLibsDir)

    doLast {
        val unsupported = envoixAndroidAbis.filterNot { it in envoixRustTargets }
        require(unsupported.isEmpty()) { "Unsupported Android ABI(s): ${unsupported.joinToString()}" }

        val cargoArgs = mutableListOf("ndk")
        envoixAndroidAbis.forEach { abi ->
            cargoArgs += listOf("-t", abi)
        }
        cargoArgs +=
            listOf(
                "--platform",
                envoixAndroidApiLevel.toString(),
                "build",
                "--release",
                "-p",
                "envoix-android-jni",
            )

        exec {
            workingDir =
                rootProject.layout.projectDirectory
                    .dir("..")
                    .asFile
            commandLine("cargo", *cargoArgs.toTypedArray())
        }

        delete(generatedJniLibsDir)
        envoixAndroidAbis.forEach { abi ->
            val rustTarget = envoixRustTargets.getValue(abi)
            val sharedLibrary =
                rootProject.layout.projectDirectory
                    .file("../target/$rustTarget/release/libenvoix_jni.so")
                    .asFile
            require(sharedLibrary.isFile) {
                "cargo-ndk did not produce ${sharedLibrary.absolutePath}"
            }
            copy {
                from(sharedLibrary)
                into(generatedJniLibsDir.map { it.dir(abi) })
            }
        }
    }
}

android {
    namespace = "dev.envoix.app"
    compileSdk = 34
    testBuildType =
        providers
            .gradleProperty("envoix.testBuildType")
            .orElse("debug")
            .get()

    defaultConfig {
        applicationId = "dev.envoix.app"
        minSdk = 29 // Android 10: scoped storage + MediaStore.Downloads
        targetSdk = 34
        versionCode = 2
        versionName = "0.2.0"
        buildConfigField("String", "GIT_COMMIT", "\"$gitCommit\"")
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            // Ship only ABIs that the JNI build task produced.
            abiFilters += envoixAndroidAbis
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

    sourceSets.getByName("main") {
        jniLibs.setSrcDirs(listOf(generatedJniLibsDir.get().asFile))
    }

    // Keep the generated JNI core as a regular packaged native library.
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }
}

tasks.configureEach {
    if (name.startsWith("merge") && name.endsWith("JniLibFolders")) {
        dependsOn(buildEnvoixJniAndroid)
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
    androidTestImplementation("androidx.test:core:1.6.1")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test:rules:1.6.1")
    androidTestImplementation("androidx.test.uiautomator:uiautomator:2.3.0")

    // JVM unit tests (the report byte-budget / head-tail logic is pure).
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.8.1")
}
