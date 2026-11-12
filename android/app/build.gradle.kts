plugins { id("com.android.application"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "dev.secrettunnel.android"
    compileSdk = 34
    ndkVersion = "27.0.12077973"
    defaultConfig {
        applicationId = "dev.secrettunnel.android"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        minSdk = 26
        targetSdk = 34
        versionCode = System.getenv("ST_VERSION_CODE")?.toIntOrNull() ?: 32000
        versionName = System.getenv("ST_VERSION_NAME") ?: "0.3.20"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    val keyPath = System.getenv("ANDROID_KEYSTORE_FILE")
    if (!keyPath.isNullOrBlank()) signingConfigs.create("release") {
        storeFile = file(keyPath)
        storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
        keyAlias = System.getenv("ANDROID_KEY_ALIAS")
        keyPassword = System.getenv("ANDROID_KEY_PASSWORD")
    }
    buildTypes { getByName("release") {
        isMinifyEnabled = false
        if (!keyPath.isNullOrBlank()) signingConfig = signingConfigs.getByName("release")
    } }
    packaging { jniLibs { useLegacyPackaging = false; keepDebugSymbols += "**/*.so" } }
}
dependencies {
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
}
tasks.register("checkNativeLibraries") {
    doLast {
        listOf("arm64-v8a", "x86_64").forEach { abi ->
            listOf("libsecret_tunnel_android.so", "libhev-socks5-tunnel.so").forEach { name ->
                check(file("src/main/jniLibs/$abi/$name").isFile) { "Missing $abi/$name. Run scripts/build-android.sh first." }
            }
        }
    }
}
tasks.named("preBuild").configure { dependsOn("checkNativeLibraries") }
tasks.register("checkReleaseSigning") {
    doLast {
        listOf("ANDROID_KEYSTORE_FILE", "ANDROID_KEYSTORE_PASSWORD", "ANDROID_KEY_ALIAS", "ANDROID_KEY_PASSWORD").forEach {
            check(!System.getenv(it).isNullOrBlank()) { "Release signing requires $it; unsigned/debug-signed release APKs are not published." }
        }
    }
}
tasks.matching { it.name == "preReleaseBuild" }.configureEach { dependsOn("checkReleaseSigning") }
