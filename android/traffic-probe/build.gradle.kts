plugins { id("com.android.application") }
android {
    namespace = "dev.secrettunnel.probe"
    compileSdk = 34
    defaultConfig { applicationId = "dev.secrettunnel.probe"; minSdk = 26; targetSdk = 34; versionCode = 1; versionName = "1" }
    flavorDimensions += "route"
    productFlavors {
        create("selected") { dimension = "route" }
        create("direct") { dimension = "route"; applicationId = "dev.secrettunnel.directprobe" }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
}
