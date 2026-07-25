plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jlleitschuh.gradle.ktlint")
}

android {
    namespace = "app.envoix.host"
    compileSdk = 34

    defaultConfig {
        minSdk = 29 // Android 10: scoped storage + MediaStore.Downloads
        targetSdk = 34
        versionCode = 1
        versionName = "0.2.0"
        ndk {
            // Only the ABIs the Rust host is cross-compiled for.
            abiFilters += listOf("x86_64", "arm64-v8a")
        }
    }

    // The identifier catalog extracts one full applicationId per flavor
    // (gradle-variant-application-id); keep each id explicit and complete.
    flavorDimensions += "endpoint"
    productFlavors {
        create("dev") {
            dimension = "endpoint"
            applicationId = "app.envoix.host.dev"
        }
        create("prod") {
            dimension = "endpoint"
            applicationId = "app.envoix.host"
        }
        create("qa") {
            dimension = "endpoint"
            applicationId = "app.envoix.host.test"
        }
    }

    buildTypes {
        release {
            // BN5 owns release trust/signing; nothing release-shaped ships
            // from BN4. Debug builds only.
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    sourceSets {
        getByName("main") {
            kotlin.srcDir("src/main/kotlin")
            // Populated by scripts/build-jni-libs.sh (cargo-ndk); gradle
            // never invokes cargo, so the two toolchains stay decoupled.
            jniLibs.srcDir("src/main/jniLibs")
        }
        // The e2e instrumentation bridge exists only in debug; release
        // compiles the no-op twin.
        getByName("debug") { kotlin.srcDir("src/debug/kotlin") }
        getByName("release") { kotlin.srcDir("src/release/kotlin") }
    }
}

dependencies {
    // FileProvider only; the host app has no UI (F1/F2 add frontends).
    implementation("androidx.core:core:1.13.1")
}

/** The one library the app loads; anything else in jniLibs is dead weight. */
val hostSoname = "libenvoix_host_android.so"
val hostAbis = listOf("arm64-v8a", "x86_64")

/**
 * Guards the packaged native payload. cargo-ndk copies every .so it finds, so
 * scripts/build-jni-libs.sh curates the directory and this task refuses to
 * assemble an APK that carries strays. It also warns when jniLibs is older
 * than the Rust sources — gradle never invokes cargo, so a stale .so would
 * otherwise ship silently.
 */
val verifyJniLibs =
    tasks.register("verifyJniLibs") {
        doLast {
            val repositoryRoot = rootProject.projectDir.parentFile.parentFile.parentFile
            val newestSource =
                fileTree(repositoryRoot) {
                    include("crates/**/*.rs", "hosts/**/*.rs")
                }.files
                    .maxOfOrNull { it.lastModified() } ?: 0L
            for (abi in hostAbis) {
                val directory = file("src/main/jniLibs/$abi")
                if (!directory.isDirectory) {
                    throw GradleException("$directory is missing: run scripts/build-jni-libs.sh")
                }
                val libraries =
                    directory
                        .listFiles { candidate -> candidate.name.endsWith(".so") }
                        .orEmpty()
                        .map { it.name }
                        .sorted()
                if (libraries != listOf(hostSoname)) {
                    throw GradleException(
                        "$abi jniLibs must contain exactly [$hostSoname], found $libraries: " +
                            "re-run scripts/build-jni-libs.sh",
                    )
                }
                if (newestSource > file("$directory/$hostSoname").lastModified()) {
                    logger.warn(
                        "warning: $abi/$hostSoname is older than the Rust sources; " +
                            "re-run scripts/build-jni-libs.sh",
                    )
                }
            }
        }
    }

tasks.named("preBuild") { dependsOn(verifyJniLibs) }
