import java.security.MessageDigest
import java.util.Properties
import java.util.zip.ZipEntry
import java.util.zip.ZipFile
import java.util.zip.ZipOutputStream

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jlleitschuh.gradle.ktlint")
    // Compiles the Dart in ../.. and packages the engine. Must come last: it
    // reads the android extension the plugins above install.
    id("dev.flutter.flutter-gradle-plugin")
}

/** The Flutter package whose Dart this app runs. */
flutter {
    source = "../.."
}

/**
 * ktlint judges the Kotlin PEOPLE write. `com/envoix/bindings` is a link to a
 * generated artifact whose formatting is the emitter's, and whose real gate is
 * far stronger than a style rule: `generated_artifacts_match_capability_schema`
 * fails on a single byte of difference from what the schema emits. Reformatting
 * it is not possible (the drift gate would reject the result) and teaching the
 * emitter one linter's preferences would be a formatter shaping a contract.
 */
ktlint {
    filter {
        exclude { it.file.absolutePath.contains("/com/envoix/bindings/") }
    }
}

/** The repository root; this project lives at apps/envoix-flutter/android/app. */
val repositoryRoot: File = rootProject.projectDir.parentFile.parentFile.parentFile

/**
 * The release policy, as the ONE flat projection the packaging side is allowed
 * to read. It is generated from `registry/release-ledger.toml` and parsed here
 * by `java.util.Properties` — a real parser over a document with no nesting, so
 * this build script cannot resolve a smuggled table into a different value than
 * the Rust gate does. `xtask release-gate` re-derives this text from the ledger
 * and fails on any divergence, so a hand-edited copy is a violation rather than
 * an invisible second opinion.
 */
val policyFile = File(repositoryRoot, "registry/release-policy.properties")
val releasePolicy =
    Properties().apply {
        if (!policyFile.isFile) {
            throw GradleException("missing $policyFile: run scripts/build-jni-libs.sh")
        }
        policyFile.inputStream().use(::load)
    }

fun policy(key: String): String = releasePolicy.getProperty(key) ?: throw GradleException("the release policy has no $key")

fun policyList(key: String): List<String> = policy(key).split(',').filter(String::isNotEmpty)

/** The one reviewed identity for the toolchain that produces the native payload. */
val nativeToolchainFile = File(repositoryRoot, "registry/android-native-toolchain.properties")
val nativeToolchain =
    Properties().apply {
        if (!nativeToolchainFile.isFile) {
            throw GradleException("missing $nativeToolchainFile")
        }
        nativeToolchainFile.inputStream().use(::load)
    }

fun nativeToolchain(key: String): String =
    nativeToolchain.getProperty(key)
        ?: throw GradleException("the native toolchain has no $key")

val expectedSigner = policy("signer_sha256")
val requiredAbis = policyList("required_abis")

/** The one library this repository BUILDS; anything else in jniLibs is dead weight. */
val hostSoname = policy("native_library")

/** Libraries the release packages but does not build, by soname. */
val bundledLibraries = policyList("bundled_libraries")

/** The complete exported surface a release payload may have, in both directions. */
val allowedNativeSymbols = policyList("allowed_native_symbols").toSet()
val forbiddenNativeSymbols = policyList("forbidden_native_symbols")
val allowedPackageEntries = policyList("allowed_package_entries")

/** The container entries an app bundle may carry; its app content is judged as an APK's. */
val allowedBundleEntries = policyList("allowed_bundle_entries")

/**
 * What the packaged payload is built from. The Rust gate digests exactly these
 * globs into `sources_sha256`; this build reads the same list so the two cannot
 * answer "what determines the payload" differently.
 */
val payloadSources = policyList("payload_sources")
val recordedPayloadSources = policy("payload_sources_sha256")
val allowedPermissions = policyList("allowed_permissions").toSet()

/** Every ABI Android defines. The policy names which of them a release ships. */
val ANDROID_ABIS = setOf("armeabi-v7a", "arm64-v8a", "x86", "x86_64")
val forbiddenManifestMarkers = policyList("forbidden_manifest_markers")
val forbiddenReleaseClasses = policyList("forbidden_release_classes")

/** `(applicationId, versionCode)` pairs that have already been released. */
val releasedVersions = policyList("released").toSet()

/**
 * The typed distribution decision, never the spelling of the enum variant: the
 * Rust side owns what "public" implies and projects only the consequence.
 */
val trustRootRequired = policy("trust_root_required").toBoolean()
val declaredTrustRoot = policy("identity_trust_root")
val compiledPackageVersion = policy("identity_package_version")
val compiledBuildManifest = policy("build_manifest_sha256")

/**
 * Release signing credentials. They live in ~/.gradle/gradle.properties,
 * OUTSIDE this (public) repository, and never appear in any file inside it.
 * When any of them is absent the release signing config is simply not created:
 * a release build then fails in `requireReleaseSigning` rather than silently
 * degrading to the debug key. Debug builds are unaffected.
 */
val signingPropertyNames =
    listOf(
        "envoixReleaseStoreFile",
        "envoixReleaseKeyAlias",
        "envoixReleaseStorePassword",
        "envoixReleaseKeyPassword",
    )
val signingProperties = signingPropertyNames.associateWith { providers.gradleProperty(it).orNull }
val missingSigningProperties = signingPropertyNames.filter { signingProperties[it].isNullOrBlank() }

android {
    namespace = "app.envoix.host"
    compileSdk = 34

    defaultConfig {
        minSdk = 29 // Android 10: scoped storage + MediaStore.Downloads
        targetSdk = 34
        versionCode = 1
        // Catalogued as android.version_name and bound to the Cargo workspace
        // version, so the two literals cannot drift apart in silence; the
        // release assertions re-check the agreement on the packaged artifact.
        versionName = "0.2.0"
        ndk {
            // Only the ABIs the release policy requires.
            abiFilters += requiredAbis
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

    signingConfigs {
        if (missingSigningProperties.isEmpty()) {
            create("release") {
                storeFile = File(signingProperties.getValue("envoixReleaseStoreFile")!!)
                storePassword = signingProperties.getValue("envoixReleaseStorePassword")
                keyAlias = signingProperties.getValue("envoixReleaseKeyAlias")
                keyPassword = signingProperties.getValue("envoixReleaseKeyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Never `signingConfigs.getByName("debug")`: an unsigned release is
            // a loud failure, a debug-signed one would be a silent lie.
            signingConfig = signingConfigs.findByName("release")
        }
    }

    packaging {
        jniLibs {
            // The payload record accounts for the bytes cargo-ndk produced, so
            // the packaged file must BE those bytes. AGP strips a native
            // library on its way into the archive, which would leave the
            // shipped artifact impossible to tie back to the sources it was
            // built from — the one thing that record exists to state.
            keepDebugSymbols += "**/$hostSoname"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    // `ndk.abiFilters` governs what this project BUILDS. It does not govern
    // what a dependency's AAR already contains: CameraX ships
    // `libimage_processing_util_jni.so` for four ABIs, and armeabi-v7a and x86
    // reached the archive past that filter. The release claims exactly two
    // ABIs, so the packager drops every other one — derived by subtracting the
    // policy's list from the ABIs Android defines, never by naming two here.
    packaging {
        jniLibs {
            for (abi in ANDROID_ABIS - requiredAbis.toSet()) {
                excludes += "lib/$abi/**"
            }
        }
    }

    sourceSets {
        getByName("main") {
            kotlin.srcDir("src/main/kotlin")
        }
        // The instrumentation seam is shaped by source set: only debug has a
        // bridge class and only debug binds the JNI instrumentation lane.
        // jniLibs is per build type for the same reason: the debug payload is
        // built WITH the host crate's `e2e-instrumentation` feature and the
        // release payload without it, so the instrumentation entry points are
        // not merely unbound in a release artifact, they were never compiled.
        getByName("debug") {
            kotlin.srcDir("src/debug/kotlin")
            jniLibs.srcDir("src/debug/jniLibs")
        }
        getByName("release") {
            kotlin.srcDir("src/release/kotlin")
            jniLibs.srcDir("src/release/jniLibs")
        }
    }
}

/** apksigner and aapt2 ship with the build tools this project already resolves. */
val buildToolsDirectory = File(android.sdkDirectory, "build-tools/${android.buildToolsVersion}")

/** keytool reports an app bundle's JAR signer; it ships with the JDK gradle runs on. */
val keytoolExecutable = File(System.getProperty("java.home"), "bin/keytool")

/**
 * `llvm-nm` reads a packaged library's dynamic symbol table, which is the
 * precise statement about what an artifact exports. It ships with the NDK that
 * cross-compiled the payload, so it is located there rather than on PATH.
 */
val llvmNmExecutable: File by lazy {
    val expectedRevision = nativeToolchain("android_ndk_revision")
    val ndk =
        System.getenv("ANDROID_NDK_HOME")?.let(::File)?.takeIf(File::isDirectory)
            ?: File(android.sdkDirectory, "ndk/$expectedRevision")
    val observedRevision =
        File(ndk, "source.properties")
            .takeIf(File::isFile)
            ?.readLines()
            ?.firstOrNull { it.substringBefore('=').trim() == "Pkg.Revision" }
            ?.substringAfter('=')
            ?.trim()
    if (observedRevision != expectedRevision) {
        throw GradleException(
            "Android NDK at $ndk is ${observedRevision ?: "unidentified"}, " +
                "expected $expectedRevision from $nativeToolchainFile",
        )
    }
    File(ndk, "toolchains/llvm/prebuilt")
        .listFiles(File::isDirectory)
        .orEmpty()
        .map { host -> File(host, "bin/llvm-nm") }
        .firstOrNull(File::canExecute)
        ?: throw GradleException("$ndk carries no llvm-nm toolchain")
}

fun sha256(bytes: ByteArray): String =
    MessageDigest
        .getInstance("SHA-256")
        .digest(bytes)
        .joinToString("") { byte -> "%02x".format(byte) }

/**
 * The Rust gate's source digest, reproduced from the SAME payload_sources
 * enumeration. Paths and per-file digests are part of the index, so renames,
 * additions and recipe/toolchain edits all invalidate the recorded payload by
 * content rather than by a timestamp accident.
 */
fun payloadSourceFiles(): Map<String, File> {
    val files = sortedMapOf<String, File>()
    for (pattern in payloadSources) {
        val matched = fileTree(repositoryRoot) { include(pattern) }.files.filter(File::isFile)
        if (matched.isEmpty()) {
            throw GradleException("payload_sources pattern \"$pattern\" names no file")
        }
        for (file in matched) {
            files[file.relativeTo(repositoryRoot).invariantSeparatorsPath] = file
        }
    }
    return files
}

fun payloadSourcesDigest(): String {
    val index =
        buildString {
            for ((path, file) in payloadSourceFiles()) {
                append(path)
                append(' ')
                append(sha256(file.readBytes()))
                append('\n')
            }
        }
    return sha256(index.toByteArray(Charsets.UTF_8))
}

/** Runs one build tool and returns everything it printed. */
fun toolOutput(
    tool: File,
    vararg arguments: String,
): String {
    if (!tool.canExecute()) {
        throw GradleException("$tool is not available")
    }
    val process =
        ProcessBuilder(listOf(tool.absolutePath) + arguments)
            .redirectErrorStream(true)
            .start()
    val output = process.inputStream.bufferedReader().readText()
    if (process.waitFor() != 0) {
        throw GradleException("${tool.name} ${arguments.joinToString(" ")} failed:\n$output")
    }
    return output
}

/**
 * The symbols a shared object DEFINES in its dynamic symbol table — the exact
 * set a caller could bind to, where scanning the bytes could only guess.
 */
fun definedSymbols(library: File): List<String> =
    toolOutput(llvmNmExecutable, "-D", "--defined-only", library.absolutePath)
        .lineSequence()
        .mapNotNull { line -> line.trim().substringAfterLast(' ').takeIf(String::isNotEmpty) }
        .toList()

/**
 * Guards one build type's packaged native payload against the record
 * `scripts/build-jni-libs.sh` wrote. Gradle never invokes cargo, so without
 * this the packaged `.so` would be an unaccountable prebuilt: a HARD failure
 * here is what keeps "the sources declare X" and "the shipped binary was built
 * from X" the same statement.
 */
fun registerJniLibsGuard(buildType: String) =
    tasks.register("verify${buildType.replaceFirstChar(Char::uppercase)}JniLibs") {
        inputs.file(policyFile)
        // Materialise exact files here. Declaring a fileTree rooted at the
        // repository makes Gradle conservatively treat every task output under
        // that root as overlapping even when no include pattern matches it.
        inputs.files(payloadSourceFiles().values)
        doLast {
            val observedPayloadSources = payloadSourcesDigest()
            if (observedPayloadSources != recordedPayloadSources) {
                throw GradleException(
                    "$buildType payload sources hash to $observedPayloadSources, but the payload " +
                        "record accounts for $recordedPayloadSources: re-run scripts/build-jni-libs.sh",
                )
            }
            for (abi in requiredAbis) {
                val directory = file("src/$buildType/jniLibs/$abi")
                val libraries =
                    directory
                        .listFiles { candidate -> candidate.name.endsWith(".so") }
                        .orEmpty()
                        .map { it.name }
                        .sorted()
                if (libraries != listOf(hostSoname)) {
                    throw GradleException(
                        "$buildType $abi jniLibs must contain exactly [$hostSoname], found $libraries: " +
                            "re-run scripts/build-jni-libs.sh",
                    )
                }
                val library = File(directory, hostSoname)
                val recorded = policy("payload_${buildType}_${abi}_sha256")
                val observed = sha256(library.readBytes())
                if (observed != recorded) {
                    throw GradleException(
                        "$buildType/$abi/$hostSoname hashes to $observed, but the payload record " +
                            "accounts for $recorded: re-run scripts/build-jni-libs.sh",
                    )
                }
            }
        }
    }

val jniLibsGuards = listOf("debug", "release").associateWith { registerJniLibsGuard(it) }

androidComponents.onVariants { variant ->
    val guard = jniLibsGuards[variant.buildType] ?: return@onVariants
    val prepare = "pre${variant.name.replaceFirstChar(Char::uppercase)}Build"
    tasks.matching { it.name == prepare }.configureEach { dependsOn(guard) }
}

/**
 * A release artifact must be signed by the real key or not exist. This runs
 * before packaging, so an unconfigured machine fails immediately instead of
 * producing something nobody can trust — and it guards the app bundle exactly
 * as it guards the APK.
 */
val requireReleaseSigning =
    tasks.register("requireReleaseSigning") {
        doLast {
            if (missingSigningProperties.isNotEmpty()) {
                throw GradleException(
                    "release signing is not configured: set " +
                        missingSigningProperties.joinToString() +
                        " in ~/.gradle/gradle.properties (never inside this repository)",
                )
            }
            if (!File(signingProperties.getValue("envoixReleaseStoreFile")!!).isFile) {
                throw GradleException("the configured release keystore does not exist")
            }
            if (android.buildTypes
                    .getByName("release")
                    .signingConfig
                    ?.name != "release"
            ) {
                throw GradleException("the release build type is not bound to the release signing identity")
            }
        }
    }

/** Where one artifact's packaging facts land for the Rust gate to re-judge. */
fun releaseTrustFacts(
    variant: String,
    kind: String,
): File =
    File(
        layout.buildDirectory
            .dir("outputs/envoix-release-trust")
            .get()
            .asFile,
        "$variant-$kind.toml",
    )

/**
 * Empties the facts directory once per build, so a flavor that was not built
 * this time cannot leave a stale green verdict behind for the Rust gate. Each
 * facts file is a declared OUTPUT of the task that writes it, so emptying the
 * directory also takes every release packaging task out of date: what the gate
 * judges is always what this build produced.
 */
val prepareReleaseTrust =
    tasks.register("prepareReleaseTrust") {
        doLast {
            layout.buildDirectory
                .dir("outputs/envoix-release-trust")
                .get()
                .asFile
                .deleteRecursively()
        }
    }

/** The signer certificates an artifact carries, lowercase hex, in signer order. */
fun packagedSigners(
    artifact: File,
    kind: String,
): List<String> =
    if (kind == "apk") {
        Regex("Signer #(\\d+) certificate SHA-256 digest: ([0-9a-fA-F]{64})")
            .findAll(
                toolOutput(
                    File(buildToolsDirectory, "apksigner"),
                    "verify",
                    "--print-certs",
                    artifact.absolutePath,
                ),
            ).associate { match -> match.groupValues[1] to match.groupValues[2].lowercase() }
            .toSortedMap()
            .values
            .toList()
    } else {
        // An app bundle is JAR-signed, which apksigner does not read.
        toolOutput(keytoolExecutable, "-printcert", "-jarfile", artifact.absolutePath)
            .split("Signer #")
            .drop(1)
            .mapNotNull { signer ->
                Regex("SHA256:\\s*([0-9A-Fa-f:]{95})")
                    .find(signer)
                    ?.groupValues
                    ?.get(1)
                    ?.replace(":", "")
                    ?.lowercase()
            }
    }

/** `*` stands for any run of characters inside one path segment. */
fun matchesPattern(
    entry: String,
    pattern: String,
): Boolean {
    val segments = entry.split('/')
    val globs = pattern.split('/')
    return segments.size == globs.size &&
        segments.zip(globs).all { (segment, glob) ->
            Regex(glob.split('*').joinToString(".*") { Regex.escape(it) }).matches(segment)
        }
}

/**
 * The reviewed-surface name an archive entry carries, or null when the entry
 * belongs to the container rather than to the app.
 *
 * An APK is the surface. A bundle keeps the same app content under
 * module-scoped prefixes, so the prefix is stripped and ONE reviewed list judges
 * both shapes; anything else in a bundle is bundletool's own metadata. The Rust
 * gate states the same mapping and re-judges every artifact this task writes
 * facts about, so it is the authority and this is the fail-early mirror.
 */
fun surfaceEntry(
    entry: String,
    kind: String,
): String? {
    if (kind == "apk") {
        return entry
    }
    for (prefix in listOf("base/dex/", "base/manifest/", "base/root/")) {
        if (entry.startsWith(prefix)) {
            return entry.removePrefix(prefix)
        }
    }
    if (!entry.startsWith("base/")) {
        return null
    }
    val rest = entry.removePrefix("base/")
    return rest.takeIf { it.substringBefore('/') in listOf("assets", "lib", "res") }
}

/** Is this entry claimed by the list that governs the shape it is in? */
fun entryAllowed(
    entry: String,
    kind: String,
): Boolean {
    val surface = surfaceEntry(entry, kind)
    val patterns = if (surface == null) allowedBundleEntries else allowedPackageEntries
    return patterns.any { pattern -> matchesPattern(surface ?: entry, pattern) }
}

/**
 * Wraps a packaged app manifest in the one-entry container aapt2 insists on, so
 * the same reader handles an APK's binary XML and a bundle's proto XML.
 */
fun manifestContainer(
    scratch: File,
    bytes: ByteArray,
): File {
    val container = File(scratch, "manifest-container.zip")
    ZipOutputStream(container.outputStream()).use { archive ->
        archive.putNextEntry(ZipEntry("AndroidManifest.xml"))
        archive.write(bytes)
        archive.closeEntry()
    }
    return container
}

/**
 * A complete PEM header: the five dashes, a label, and five dashes again. The
 * label class carries digits because real ones do — `PKCS7`, `X509 CRL`,
 * `PKCS12`, `SSH2 PUBLIC KEY` — and a fix for a false positive that quietly
 * stopped catching those would be worse than the false positive.
 */
val pemHeader = Regex("^-----BEGIN [A-Z0-9 ]{1,48}-----")

/**
 * PEM trust material, whether it sits in an asset, a resource or a string pool.
 *
 * The LABEL and its closing dashes are what separate trust material from a
 * mention of it. `libflutter.so` carries the bare string `"-----BEGIN "`
 * because dart:io's own PEM reader is compiled into it, next to `"-----END "`
 * and `"OPEN "` in the same constant pool — a prefix-only test reads that as a
 * packaged root certificate. Every label still matches, so a CERTIFICATE, a
 * PRIVATE KEY or a PUBLIC KEY block is caught exactly as before.
 */
fun carriesPem(bytes: ByteArray): Boolean =
    listOf(Charsets.US_ASCII, Charsets.UTF_16LE, Charsets.UTF_16BE).any { charset ->
        val needle = "-----BEGIN ".toByteArray(charset)
        // The longest header this can be: the prefix, 48 label characters and
        // the closing dashes, in whatever width the charset uses.
        val window = needle.size + 53 * (needle.size / "-----BEGIN ".length)
        (0..bytes.size - needle.size).any { start ->
            needle.indices.all { index -> bytes[start + index] == needle[index] } &&
                pemHeader.containsMatchIn(
                    String(bytes, start, minOf(window, bytes.size - start), charset),
                )
        }
    }

/** Labels a packaged trust root could realistically carry. */
val pemLabels =
    listOf(
        "CERTIFICATE",
        "PRIVATE KEY",
        "PUBLIC KEY",
        "RSA PRIVATE KEY",
        "PKCS7",
        "PKCS12",
        "X509 CERTIFICATE",
        "X509 CRL",
        "SSH2 PUBLIC KEY",
    )

/**
 * Proves the byte scan still discriminates before it is trusted with 25 MB of
 * engine. A build script has no test harness, so the predicate proves itself:
 * every label, in every shape a packaged file could hold one, must be caught,
 * and the bare prefix that dart:io's PEM *parser* compiles into libflutter.so
 * must not be.
 */
fun assertPemScanDiscriminates() {
    for (label in pemLabels) {
        val header = "-----BEGIN $label-----"
        val shapes =
            mapOf(
                "verbatim" to header.toByteArray(),
                "CRLF" to "$header\r\nQUJD\r\n-----END $label-----\r\n".toByteArray(),
                "UTF-16LE" to header.toByteArray(Charsets.UTF_16LE),
                "UTF-16BE" to header.toByteArray(Charsets.UTF_16BE),
                "mid-file" to ("x".repeat(4096) + header + "\n").toByteArray(),
                "single-line" to "trustRoot=$header QUJD -----END $label-----".toByteArray(),
            )
        for ((shape, probe) in shapes) {
            if (!carriesPem(probe)) {
                throw GradleException("the packaged trust-material scan misses $label ($shape)")
            }
        }
    }
    // A mention of PEM is not PEM: the parser's constant pool holds the bare
    // prefix, and a header with no label is not a header.
    for (benign in listOf("literal: \"-----BEGIN \" OPEN  -----END  ", "-----BEGIN -----")) {
        if (carriesPem(benign.toByteArray())) {
            throw GradleException("the packaged trust-material scan reads $benign as trust material")
        }
    }
}

/**
 * Verifies one packaged release artifact against the policy and records what it
 * saw for `cargo run -p xtask -- release-gate`, which re-reads and re-hashes the
 * same file before judging the same facts.
 *
 * This runs as the artifact-producing task's own final action, not as a sibling
 * task: there is no `-x` that drops the assertions and keeps the artifact.
 */
fun verifyReleaseArtifact(
    flavor: String,
    variant: String,
    kind: String,
) {
    val directory =
        layout.buildDirectory
            .dir(if (kind == "apk") "outputs/apk/$flavor/release" else "outputs/bundle/$variant")
            .get()
            .asFile
    val suffix = if (kind == "apk") ".apk" else ".aab"
    val artifact =
        directory
            .listFiles { candidate -> candidate.name.endsWith(suffix) }
            .orEmpty()
            .singleOrNull()
            ?: throw GradleException("expected exactly one release $suffix in $directory")
    assertPemScanDiscriminates()
    val prefix = if (kind == "apk") "" else "base/"
    val scratch =
        layout.buildDirectory
            .dir("release-trust-scratch/$variant-$kind")
            .get()
            .asFile
    scratch.deleteRecursively()
    scratch.mkdirs()

    val abis = sortedSetOf<String>()
    val entries = mutableListOf<String>()
    val payload = mutableListOf<Triple<String, String, List<String>>>()
    val trustMaterial = mutableListOf<String>()
    val releaseClasses = sortedSetOf<String>()
    var shippedManifest: String? = null
    var manifestBytes: ByteArray? = null
    ZipFile(artifact).use { archive ->
        for (entry in archive.entries()) {
            val name = entry.name
            entries += name
            val bytes = archive.getInputStream(entry).use { it.readBytes() }
            if (carriesPem(bytes)) {
                trustMaterial += name
            }
            when (name) {
                "${prefix}assets/envoix-build-manifest.json" -> shippedManifest = sha256(bytes)
                if (kind == "apk") "AndroidManifest.xml" else "base/manifest/AndroidManifest.xml" ->
                    manifestBytes = bytes
            }
            if (name.endsWith(".dex")) {
                releaseClasses +=
                    forbiddenReleaseClasses.filter { forbidden ->
                        String(bytes, Charsets.US_ASCII).contains(forbidden)
                    }
            }
            // EVERY shared object, wherever it sits: one dropped in assets/ and
            // System.load()ed is a native entry point just the same.
            if (!name.endsWith(".so")) {
                continue
            }
            name.removePrefix(prefix).split('/').let { segments ->
                if (segments.size == 3 && segments[0] == "lib") {
                    abis += segments[1]
                }
            }
            val extracted = File(scratch, name.replace('/', '-'))
            extracted.writeBytes(bytes)
            payload += Triple(name, sha256(bytes), definedSymbols(extracted))
        }
    }

    // The packaged app manifest is the artifact's OWN declaration, so the
    // version, the applicationId and any debug marker are read back out of the
    // bytes it ships rather than from what the build script or AGP's metadata
    // claims. An app bundle keeps its manifest proto-encoded under base/, which
    // aapt2 reads once it is handed the entry under its conventional name.
    val manifest =
        toolOutput(
            File(buildToolsDirectory, "aapt2"),
            "dump",
            "xmltree",
            "--file",
            "AndroidManifest.xml",
            manifestContainer(
                scratch,
                manifestBytes
                    ?: throw GradleException("$variant packages no app manifest"),
            ).absolutePath,
        )

    fun manifestAttribute(
        name: String,
        pattern: String,
    ): String =
        Regex("$name(?:\\(0x[0-9a-f]+\\))?=$pattern")
            .find(manifest)
            ?.groupValues
            ?.get(1)
            ?: throw GradleException("$variant packages a manifest that declares no $name")

    val versionCode = manifestAttribute("versionCode", "([0-9]+)").toLong()
    val versionName = manifestAttribute("versionName", "\"([^\"]*)\"")
    val applicationId = manifestAttribute("package", "\"([^\"]*)\"")
    val manifestMarkers = forbiddenManifestMarkers.filter(manifest::contains)
    // Every permission the PACKAGED manifest requests, read from the same
    // aapt2 dump the markers are: the source manifest is not the artifact, and
    // a merged dependency manifest can add a permission nobody wrote here.
    val permissions =
        Regex("uses-permission[\\s\\S]*?android:name(?:\\(0x[0-9a-f]+\\))?=\"([^\"]+)\"")
            .findAll(manifest)
            .map { it.groupValues[1] }
            .toSortedSet()
    val signers = packagedSigners(artifact, kind)

    val artifactPath = repositoryRoot.toURI().relativize(artifact.toURI()).path
    val facts = releaseTrustFacts(variant, kind)
    facts.parentFile.mkdirs()
    facts.writeText(
        buildString {
            appendLine("# @generated by the $variant $kind packaging assertions.")
            appendLine("[facts]")
            appendLine("variant = \"$variant\"")
            appendLine("kind = \"$kind\"")
            appendLine("application_id = \"$applicationId\"")
            appendLine("artifact = \"$artifactPath\"")
            appendLine("artifact_sha256 = \"${sha256(artifact.readBytes())}\"")
            appendLine("version_code = $versionCode")
            appendLine("version_name = \"$versionName\"")
            appendLine("signers = [${signers.joinToString { "\"$it\"" }}]")
            appendLine("abis = [${abis.joinToString { "\"$it\"" }}]")
            appendLine("entries = [${entries.sorted().joinToString { "\"$it\"" }}]")
            appendLine("permissions = [${permissions.joinToString { "\"$it\"" }}]")
            appendLine("manifest_markers = [${manifestMarkers.joinToString { "\"$it\"" }}]")
            appendLine("trust_material = [${trustMaterial.joinToString { "\"$it\"" }}]")
            appendLine(
                shippedManifest?.let { "build_manifest_sha256 = \"$it\"" }
                    ?: "# build_manifest_sha256: the artifact carries no build manifest",
            )
            appendLine("release_classes = [${releaseClasses.joinToString { "\"$it\"" }}]")
            for ((entry, digest, symbols) in payload) {
                appendLine()
                appendLine("[[facts.payload]]")
                appendLine("artifact = \"$entry\"")
                appendLine("sha256 = \"$digest\"")
                appendLine("symbols = [${symbols.joinToString { "\"$it\"" }}]")
            }
        },
    )

    val failures = mutableListOf<String>()
    if ("$applicationId:$versionCode" in releasedVersions) {
        failures += "$variant packages $applicationId versionCode $versionCode, already released"
    }
    releasedVersions
        .mapNotNull { released ->
            released.substringBeforeLast(':').takeIf { it == applicationId }?.let {
                released.substringAfterLast(':').toLong()
            }
        }.maxOrNull()
        ?.let { lastReleased ->
            if (versionCode < lastReleased) {
                failures += "$variant packages versionCode $versionCode, but $lastReleased is already released"
            }
        }
    if (versionName != compiledPackageVersion) {
        failures +=
            "$variant packages versionName $versionName, but the build compiled " +
            "package version $compiledPackageVersion"
    }
    if (signers.size != 1) {
        failures += "$variant carries ${signers.size} signers, but a release has exactly one"
    }
    for (signer in signers) {
        if (signer != expectedSigner) {
            failures += "$variant is signed by $signer, but the release identity is $expectedSigner"
        }
    }
    for (abi in requiredAbis - abis) {
        failures += "$variant ships no $abi payload, but the release requires it"
    }
    for (abi in abis - requiredAbis.toSet()) {
        failures += "$variant ships an $abi payload the release does not claim"
    }
    val expectedLibraries = requiredAbis.map { abi -> "${prefix}lib/$abi/$hostSoname" }
    // Libraries the release packages but does not build. Kept apart from the
    // payload paths, never merged into them: naming one lets it exist, and
    // nothing else.
    val bundledLibraryPaths =
        requiredAbis
            .flatMap { abi -> bundledLibraries.map { soname -> "${prefix}lib/$abi/$soname" } }
            .toSet()
    for (expected in expectedLibraries - payload.map { it.first }.toSet()) {
        failures += "$variant packages no $expected, but the release requires it"
    }
    for ((entry, digest, symbols) in payload) {
        if (entry in bundledLibraryPaths) {
            // Its BYTES, not its name. A bundled library can bind every JNI
            // method from JNI_OnLoad without exporting a single name, so a
            // soname cannot be the trust decision; the recorded digest is.
            val soname = entry.substringAfterLast('/')
            val abi = entry.split('/')[if (kind == "apk") 1 else 2]
            when (val recorded = releasePolicy.getProperty("bundled_${soname}_${abi}_sha256")) {
                null ->
                    failures +=
                        "$variant packages $entry, a bundled library whose bytes the release has " +
                        "not recorded: re-run `cargo run -p xtask -- record-bundled`"
                digest -> Unit
                else ->
                    failures +=
                        "$variant packages $entry hashing to $digest, but the bundled record " +
                        "accepts $recorded"
            }
            for (symbol in symbols) {
                if (symbol in allowedNativeSymbols) {
                    failures +=
                        "$variant packages $entry, a library the release does not build, " +
                        "which exports the payload's own entry point $symbol"
                } else if (forbiddenNativeSymbols.any(symbol::startsWith)) {
                    failures += "$variant packages $entry, which exports the debug-only symbol $symbol"
                }
            }
            continue
        }
        if (entry !in expectedLibraries) {
            failures += "$variant packages $entry, which the release does not claim"
        } else {
            val abi = entry.split('/')[if (kind == "apk") 1 else 2]
            val recorded = policy("payload_release_${abi}_sha256")
            if (digest != recorded) {
                failures +=
                    "$variant packages $entry hashing to $digest, but the payload record " +
                    "accounts for $recorded"
            }
        }
        for (symbol in symbols - allowedNativeSymbols) {
            failures +=
                if (forbiddenNativeSymbols.any(symbol::startsWith)) {
                    "$variant packages $entry, which exports the debug-only symbol $symbol"
                } else {
                    "$variant packages $entry, which exports $symbol, an entry point the release does not allow"
                }
        }
        for (symbol in allowedNativeSymbols - symbols.toSet()) {
            failures += "$variant packages $entry, which does not export the required entry point $symbol"
        }
    }
    for (entry in entries.filterNot { entryAllowed(it, kind) }) {
        failures += "$variant packages $entry, which the release surface does not allow"
    }
    when (shippedManifest) {
        null -> failures += "$variant carries no build-manifest asset to identify itself by"
        compiledBuildManifest -> Unit
        else ->
            failures +=
                "$variant ships build manifest $shippedManifest, but this build compiles $compiledBuildManifest"
    }
    // Both directions, exactly as the independent gate judges them: an
    // unreviewed permission fails, and so does a claim the artifact never makes.
    val claimedPermissions =
        allowedPermissions.map { it.replace("{application_id}", applicationId) }.toSet()
    for (permission in permissions - claimedPermissions) {
        failures += "$variant requests $permission, which the release does not claim"
    }
    for (permission in claimedPermissions - permissions) {
        failures += "$variant never requests $permission, which the release claims"
    }
    for (marker in manifestMarkers) {
        failures += "$variant declares $marker, which only a debug build may carry"
    }
    for (entry in trustMaterial) {
        failures += "$variant packages $entry, which carries PEM trust material"
    }
    for (className in releaseClasses) {
        failures += "$variant defines $className, which only a debug build may define"
    }
    if (trustRootRequired && declaredTrustRoot == "unprovisioned") {
        failures += "$variant is a public release, but the deployment trust root slot is unprovisioned"
    }
    if (failures.isNotEmpty()) {
        throw GradleException(failures.joinToString(separator = "\n- ", prefix = "release trust failed:\n- "))
    }
    // What was judged, not just that nothing failed: a rule that silently
    // stopped running reads exactly like a rule that passed.
    logger.lifecycle(
        "release trust: $variant $kind judged ${entries.size} entries, ${payload.size} libraries, " +
            "${signers.size} signers, ${abis.size} abis, ${forbiddenManifestMarkers.size} manifest markers, " +
            "${forbiddenReleaseClasses.size} release classes",
    )
}

/**
 * The Rust half of the same question, wired into the release graph: gradle owns
 * what only the packaging side can see, and this owns the identity agreement no
 * build script can compute. Release builds already require the cargo toolchain
 * that produced the payload, so this costs debug builds nothing.
 */
val releaseGate =
    tasks.register<Exec>("releaseGate") {
        workingDir = repositoryRoot
        commandLine("cargo", "run", "-q", "-p", "xtask", "--", "release-gate")
    }

androidComponents.onVariants { variant ->
    if (variant.buildType != "release") {
        return@onVariants
    }
    val name = variant.name
    val flavor = variant.flavorName.orEmpty()
    val capitalized = name.replaceFirstChar(Char::uppercase)
    afterEvaluate {
        // Every task that can emit a release artifact requires the real key —
        // the bundle path included, where an unsigned .aab used to build fine.
        for (producer in listOf(
            "package$capitalized",
            "package${capitalized}Bundle",
            "sign${capitalized}Bundle",
            "package${capitalized}UniversalApk",
        )) {
            tasks.named(producer) { dependsOn(requireReleaseSigning) }
        }
        // The assertions belong to the tasks that PRODUCE the artifacts, so
        // they cannot be excluded without also excluding the artifact.
        tasks.named("package$capitalized") {
            dependsOn(prepareReleaseTrust)
            inputs.file(policyFile)
            outputs.file(releaseTrustFacts(name, "apk"))
            doLast { verifyReleaseArtifact(flavor, name, "apk") }
        }
        tasks.named("sign${capitalized}Bundle") {
            dependsOn(prepareReleaseTrust)
            inputs.file(policyFile)
            outputs.file(releaseTrustFacts(name, "bundle"))
            doLast { verifyReleaseArtifact(flavor, name, "bundle") }
        }
        releaseGate.configure {
            mustRunAfter("package$capitalized", "sign${capitalized}Bundle")
        }
        tasks.named("assemble$capitalized") { dependsOn(releaseGate) }
        tasks.named("bundle$capitalized") { dependsOn(releaseGate) }
    }
}

dependencies {
    // FileProvider only; the host app has no UI (F1/F2 add frontends).
    implementation("androidx.core:core:1.13.1")

    // The `scan_invite` capability's Android currency, and it is Android's
    // alone: CameraX for frames, ZXing to decode one. Nothing here appears in
    // the shared capability contract, which is why an Apple adapter pays in
    // AVFoundation instead and a desktop one pays nothing at all.
    //
    // `camera-view` brings `PreviewView`; `camera-lifecycle` brings the
    // `LifecycleOwner` binding that `ScanActivity` supplies from a plain
    // `LifecycleRegistry`, which is what keeps androidx `appcompat` and
    // `activity` off this list.
    implementation("androidx.camera:camera-core:1.3.4")
    implementation("androidx.camera:camera-camera2:1.3.4")
    implementation("androidx.camera:camera-lifecycle:1.3.4")
    implementation("androidx.camera:camera-view:1.3.4")
    implementation("com.google.zxing:core:3.5.3")
}
