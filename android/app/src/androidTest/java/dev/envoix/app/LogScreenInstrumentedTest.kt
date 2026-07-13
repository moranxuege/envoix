package dev.envoix.app

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LogScreenInstrumentedTest {
    @Test
    fun applicationInitializesDiagnosticsStorage() {
        ApplicationProvider.getApplicationContext<EnvoixApp>()
        Diagnostics.pendingCrash()
    }
}
