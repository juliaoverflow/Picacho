// Picacho's Android app: a NativeActivity that loads libpicacho.so (src/android.rs).
// The Rust library is built by scripts/build-android.sh into src/main/jniLibs.

import com.android.build.api.variant.FilterConfiguration

plugins {
    id("com.android.application")
}

// The version comes from Cargo.toml, so the APK always matches the crate.
val cargoVersion: String = run {
    val manifest = rootProject.file("../Cargo.toml").readText()
    Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(manifest)!!.groupValues[1]
}

// versionCode: MMmmpp + a two-digit pre-release slot (99 for a release, N for
// -beta.N / -rc.N), times ten plus an ABI digit (F-Droid's convention).
// So 0.2.0-beta.1 < 0.2.0 < 0.2.1-beta.1, and each ABI's APK has its own code.
val baseVersionCode: Int = run {
    val match = Regex("""^(\d+)\.(\d+)\.(\d+)(?:-[A-Za-z]+\.?(\d+))?""").find(cargoVersion)
        ?: error("Cannot parse the version $cargoVersion")
    val (major, minor, patch, pre) = match.destructured
    val preSlot = if (pre.isEmpty()) 99 else pre.toInt().coerceIn(1, 98)
    ((major.toInt() * 100 + minor.toInt()) * 100 + patch.toInt()) * 100 + preSlot
}
val abiDigits = mapOf("armeabi-v7a" to 1, "arm64-v8a" to 2, "x86" to 3, "x86_64" to 4)
val abis = (findProperty("picacho.abis") as String? ?: "arm64-v8a,x86_64").split(",")

android {
    namespace = "tech.pinal.picacho"
    compileSdk = 36

    defaultConfig {
        applicationId = "tech.pinal.picacho"
        minSdk = 26
        targetSdk = 36
        versionName = cargoVersion
        versionCode = baseVersionCode * 10
    }

    // Release signing comes from the environment (CI secrets); without it
    // the release APK is signed with the debug key so it still installs.
    val keystore = System.getenv("PICACHO_KEYSTORE")
    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("PICACHO_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("PICACHO_KEY_ALIAS")
                keyPassword = System.getenv("PICACHO_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    splits {
        abi {
            isEnable = true
            reset()
            include(*abis.toTypedArray())
            isUniversalApk = false
        }
    }

    packaging {
        // The Rust library is already stripped by its release profile; keep
        // debug symbols out of the APK either way.
        jniLibs.keepDebugSymbols.clear()
    }

    // F-Droid's index lists dependency metadata Google signs into APKs; leave it out.
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }
}

androidComponents {
    onVariants { variant ->
        variant.outputs.forEach { output ->
            val abi = output.filters.find { it.filterType == FilterConfiguration.FilterType.ABI }?.identifier
            val digit = abiDigits[abi] ?: 0
            output.versionCode.set(baseVersionCode * 10 + digit)
        }
    }
}
