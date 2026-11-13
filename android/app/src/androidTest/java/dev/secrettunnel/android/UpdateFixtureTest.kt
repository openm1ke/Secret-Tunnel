package dev.secrettunnel.android

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class UpdateFixtureTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext

    @Test fun downloadCurrentPublicRelease() {
        assumeTrue(InstrumentationRegistry.getArguments().getString("downloadPublicRelease") == "true")
        val release = ReleaseUpdates.latest(null)
        val apk = File.createTempFile("download-test-", ".apk", context.cacheDir)
        try {
            val asset = checkNotNull(release.apk)
            ReleaseUpdates.download(release, asset, apk, null)
            assertEquals(asset.size, apk.length())
        } finally { apk.delete() }
    }

    @Test fun launchSystemInstallerWithVerifiedFixture() {
        val version = InstrumentationRegistry.getArguments().getString("candidateVersion")
        assumeTrue(version != null)
        val candidate = File(context.cacheDir, "test-update.apk")
        assertTrue(candidate.isFile)
        val activity = instrumentation.startActivitySync(Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
        try {
            MobileUpdates.stageVerifiedApk(context, candidate, version!!)
            instrumentation.runOnMainSync { MobileUpdates.openInstaller(activity, 104) }
            val deadline = android.os.SystemClock.elapsedRealtime() + 60_000
            while (android.os.SystemClock.elapsedRealtime() < deadline) {
                // Replacing the package terminates this process. The host checks
                // the resulting package version and preserved settings via adb.
                Thread.sleep(200)
            }
            fail("Android did not replace the package")
        } finally {
            candidate.delete()
            instrumentation.runOnMainSync { activity.finish() }
        }
    }
}
