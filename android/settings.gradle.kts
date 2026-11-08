pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS); repositories { google(); mavenCentral() } }
rootProject.name = "SecretTunnelAndroid"
include(":app")
// Isolated debug clients used to verify per-UID routing on an emulator/device.
include(":traffic-probe")
