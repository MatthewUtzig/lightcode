package ai.lightcode.core.jni

/**
 * Loads the Rust-provided JNI shared library exactly once.
 */
internal object NativeLoader {
    private const val LIB_NAME = "codex_core_jni"

    @Volatile
    private var loaded = false

    fun ensureLoaded() {
        if (loaded) return
        synchronized(this) {
            if (loaded) return
            System.loadLibrary(LIB_NAME)
            loaded = true
        }
    }
}
