plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jlleitschuh.gradle.ktlint")
}

// Short git SHA embedded into BuildConfig, so uploaded logs identify the build.
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
    } catch (_: Exception) {
        "unknown"
    }

val envoixAndroidAbis = (
    providers.gradleProperty("envoix.android.abis").orNull
        ?: providers.environmentVariable("ENVOIX_ANDROID_ABIS").orNull
        ?: "arm64-v8a"
)
    .split(",")
    .map { it.trim() }
    .filter { it.isNotEmpty() }

val envoixRustTargets = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "x86_64" to "x86_64-linux-android",
)

val generatedJniLibsDir = layout.buildDirectory.dir("generated/envoix/jniLibs")

val buildEnvoixFfiAndroid by tasks.registering {
    group = "build"
    description = "Builds the UniFFI Rust core for Android and stages native libraries for packaging."

    inputs.files(
        rootProject.layout.projectDirectory.dir("../crates").asFileTree,
        rootProject.layout.projectDirectory.file("../Cargo.toml"),
        rootProject.layout.projectDirectory.file("../Cargo.lock"),
    )
    outputs.dir(generatedJniLibsDir)

    doLast {
        val unsupported = envoixAndroidAbis.filterNot { it in envoixRustTargets }
        require(unsupported.isEmpty()) { "Unsupported Android ABI(s): ${unsupported.joinToString()}" }

        val cargoArgs = mutableListOf("ndk")
        envoixAndroidAbis.forEach { abi ->
            cargoArgs += listOf("-t", abi)
        }
        cargoArgs += listOf(
            "--platform", "26",
            "rustc",
            "--release",
            "-p", "envoix-ffi",
            "--lib",
            "--crate-type", "cdylib",
        )

        exec {
            workingDir = rootProject.layout.projectDirectory.dir("..").asFile
            commandLine("cargo", *cargoArgs.toTypedArray())
        }

        delete(generatedJniLibsDir)
        envoixAndroidAbis.forEach { abi ->
            val rustTarget = envoixRustTargets.getValue(abi)
            copy {
                from(rootProject.layout.projectDirectory.file("../target/$rustTarget/release/libenvoix_ffi.so"))
                into(generatedJniLibsDir.map { it.dir(abi) })
            }
        }
    }
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
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            // Ship only ABIs that the Rust UniFFI core is built for.
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
        buildConfig = true
    }

    sourceSets.getByName("main") {
        jniLibs.setSrcDirs(listOf(generatedJniLibsDir.get().asFile))
    }

    // UniFFI's JNA loader expects native libraries to be present on disk.
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }
}

tasks.configureEach {
    if (name.startsWith("merge") && name.endsWith("JniLibFolders")) {
        dependsOn(buildEnvoixFfiAndroid)
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
    implementation("net.java.dev.jna:jna:5.12.0@aar")          // UniFFI Kotlin runtime loader
    implementation("com.google.zxing:core:3.5.3")               // QR encode + decode
    // CameraX for the custom QR scanner (preview + frame analysis)
    implementation("androidx.camera:camera-core:1.3.4")
    implementation("androidx.camera:camera-camera2:1.3.4")
    implementation("androidx.camera:camera-lifecycle:1.3.4")
    implementation("androidx.camera:camera-view:1.3.4")
    androidTestImplementation(composeBom)
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test:core:1.6.1")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test:rules:1.6.1")
    androidTestImplementation("androidx.test:runner:1.6.2")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
