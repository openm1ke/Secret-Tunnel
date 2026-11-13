package dev.secrettunnel.android

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ReleaseUpdatesTest {
    private val name = "SecretTunnel-0.3.24-android.apk"
    private val asset = JSONObject().put("name", name).put("size", 42)
        .put("browser_download_url", "$PROJECT_URL/releases/download/v0.3.24/$name")

    @Test fun publishedVersionAndProjectAssetMustMatch() {
        val json = JSONObject().put("tag_name", "v0.3.24").put("assets", JSONArray().put(asset))
        assertEquals(name, parseAppRelease(json, "0.3.24").apk?.name)
        assertThrows(IllegalArgumentException::class.java) { parseAppRelease(json, "0.3.25") }
        asset.put("browser_download_url", "https://example.org/$name")
        assertNull(parseAppRelease(json).apk)
        json.put("prerelease", true)
        assertThrows(IllegalArgumentException::class.java) { parseAppRelease(json) }
    }

    @Test fun onlyOneChecksumForTheExactFilenameIsAccepted() {
        val digest = "ab".repeat(32)
        assertEquals(digest, releaseChecksum("${digest.uppercase()}  ./$name\r\n", name))
        assertThrows(IllegalArgumentException::class.java) {
            releaseChecksum("$digest  $name.old", name)
        }
        assertThrows(IllegalArgumentException::class.java) {
            releaseChecksum("$digest  $name\n$digest *$name", name)
        }
    }

    @Test fun versionsCompareNumerically() {
        assertTrue(compareReleaseVersions("0.3.10", "0.3.9") > 0)
        assertTrue(compareReleaseVersions("0.3.24", "1.0.0") < 0)
        assertEquals(0, compareReleaseVersions("0.3.24", "0.3.24"))
    }
}
