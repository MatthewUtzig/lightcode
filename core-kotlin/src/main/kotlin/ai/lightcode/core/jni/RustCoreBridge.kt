package ai.lightcode.core.jni

/**
 * Raw JNI surface that forwards calls into the existing Rust engine.
 *
 * All inputs/outputs are JSON strings so both sides can evolve their
 * data structures independently during the migration.
 */
object RustCoreBridge {

    init {
        NativeLoader.ensureLoaded()
    }

    @JvmStatic
    external fun initialize(configJson: String)

    @JvmStatic
    external fun execute(requestJson: String): String

    @JvmStatic
    external fun shutdown()
}
