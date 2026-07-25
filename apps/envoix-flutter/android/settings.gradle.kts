pluginManagement {
    // Where the Flutter SDK is. `local.properties` is machine-local and never
    // checked in, so a fresh checkout resolves the SDK from the environment or
    // from the `flutter` on PATH rather than failing on a missing file.
    // (`pluginManagement` is evaluated before the rest of this script, so this
    // cannot be hoisted out of the block.)
    val flutterSdk: String =
        run {
            val local = File(rootDir, "local.properties")
            if (local.isFile) {
                val properties = java.util.Properties()
                local.inputStream().use(properties::load)
                properties.getProperty("flutter.sdk")?.let { return@run it }
            }
            System.getenv("FLUTTER_ROOT")?.let { return@run it }
            val executable =
                System
                    .getenv("PATH")
                    .orEmpty()
                    .split(File.pathSeparator)
                    .map { directory -> File(directory, "flutter") }
                    .firstOrNull(File::canExecute)
                    ?: throw GradleException(
                        "the Flutter SDK was not found: set flutter.sdk in local.properties, " +
                            "export FLUTTER_ROOT, or put flutter on PATH",
                    )
            executable.canonicalFile.parentFile.parentFile.path
        }

    includeBuild("$flutterSdk/packages/flutter_tools/gradle")
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

// Every plugin version is declared here rather than in the root build script:
// the Flutter tooling arrives as an included build, which puts Kotlin on the
// classpath before a project-level `plugins` block could ask for a version.
plugins {
    id("dev.flutter.flutter-plugin-loader") version "1.0.0"
    id("com.android.application") version "8.7.3" apply false
    id("org.jetbrains.kotlin.android") version "2.0.20" apply false
    id("org.jlleitschuh.gradle.ktlint") version "14.2.0" apply false
}

dependencyResolutionManagement {
    // The Flutter gradle plugin declares its engine repository on the PROJECT,
    // which FAIL_ON_PROJECT_REPOS rejects outright. PREFER_SETTINGS keeps this
    // file authoritative — a project-declared repository is ignored rather than
    // obeyed — so every repository an artifact may come from is still named
    // here, the engine artifacts included.
    repositoriesMode.set(RepositoriesMode.PREFER_SETTINGS)
    repositories {
        google()
        mavenCentral()
        maven("https://storage.googleapis.com/download.flutter.io")
    }
}

rootProject.name = "envoix-android-host"
include(":app")
