package ai.lightcode.core.engine

import ai.lightcode.core.jni.RustCoreBridge

/**
 * Kotlin-friendly façade around the JNI bindings.
 */
object CoreEngine {

    fun initialize(configJson: String) {
        RustCoreBridge.initialize(configJson)
    }

    fun execute(requestJson: String): String {
        return RustCoreBridge.execute(requestJson)
    }

    fun shutdown() {
        RustCoreBridge.shutdown()
    }
}
