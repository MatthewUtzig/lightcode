package ai.lightcode.core.engine

import ai.lightcode.core.jni.NativeLoader

object NativeTestSupport {
    fun isNativeAvailable(): Boolean =
        try {
            NativeLoader.ensureLoaded()
            true
        } catch (_: UnsatisfiedLinkError) {
            false
        }
}
