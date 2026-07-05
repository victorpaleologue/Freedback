package net.freedback.app

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.net.Uri
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.espresso.intent.Intents
import androidx.test.espresso.intent.Intents.intended
import androidx.test.espresso.intent.Intents.intending
import androidx.test.espresso.intent.matcher.IntentMatchers.hasAction
import androidx.test.espresso.intent.matcher.IntentMatchers.hasComponent
import androidx.test.espresso.intent.matcher.IntentMatchers.hasData
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.hamcrest.Matchers.allOf
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The Android share bridge, asserted at the intent level (espresso-intents):
 * firing a share intent at [ShareActivity] must forward a
 * `freedback://share?text=<urlencoded>` VIEW intent to [MainActivity] and
 * finish. The webview is deliberately NOT driven here — the native
 * (tauri-driver) suite owns UX flows; these tests own what only a device can
 * prove: real intents and the real activity lifecycle.
 */
@RunWith(AndroidJUnit4::class)
class ShareIntentTest {

    @Before
    fun setUp() {
        Intents.init()
        // Stub the forwarded intent so MainActivity (the full Tauri webview)
        // does not actually boot inside the test process — we assert on the
        // recorded intent, not on the webview.
        intending(hasAction(Intent.ACTION_VIEW))
            .respondWith(Instrumentation.ActivityResult(Activity.RESULT_OK, null))
    }

    @After
    fun tearDown() {
        Intents.release()
    }

    private fun launchShare(configure: Intent.() -> Unit): ActivityScenario<ShareActivity> {
        val intent = Intent(
            ApplicationProvider.getApplicationContext(),
            ShareActivity::class.java
        ).apply(configure)
        return ActivityScenario.launch(intent)
    }

    private fun assertForwarded(text: String) {
        intended(
            allOf(
                hasAction(Intent.ACTION_VIEW),
                hasData(Uri.parse("freedback://share?text=" + Uri.encode(text))),
                hasComponent(MainActivity::class.java.name)
            )
        )
    }

    @Test
    fun sendTextWithBarcodeForwardsAsDeepLink() {
        val scenario = launchShare {
            action = Intent.ACTION_SEND
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, "3017620422003")
        }
        assertForwarded("3017620422003")
        assertEquals(Lifecycle.State.DESTROYED, scenario.state)
    }

    @Test
    fun sendTextWithUrlForwardsAsDeepLink() {
        val scenario = launchShare {
            action = Intent.ACTION_SEND
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, "https://example.com/item/1")
        }
        assertForwarded("https://example.com/item/1")
        assertEquals(Lifecycle.State.DESTROYED, scenario.state)
    }

    @Test
    fun processTextForwardsAsDeepLink() {
        val scenario = launchShare {
            action = Intent.ACTION_PROCESS_TEXT
            type = "text/plain"
            putExtra(Intent.EXTRA_PROCESS_TEXT, "ISBN 978-0-306-40615-7" as CharSequence)
        }
        assertForwarded("ISBN 978-0-306-40615-7")
        assertEquals(Lifecycle.State.DESTROYED, scenario.state)
    }
}
