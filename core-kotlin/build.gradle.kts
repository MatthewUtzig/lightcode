plugins {
    kotlin("jvm") version "2.0.21"
    kotlin("plugin.serialization") version "2.0.21"
    id("com.gradleup.shadow") version "9.2.2"
    application
}

group = "ai.lightcode"
version = "0.1.0-SNAPSHOT"

repositories {
    mavenCentral()
}

kotlin {
    // Match the toolchain the repo already uses (JDK 21 on Arch Linux).
    jvmToolchain(21)
}

dependencies {
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")

    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.0")
}

tasks.withType<Test> {
    useJUnitPlatform()
}

tasks.jar {
    archiveBaseName.set("core-kotlin")
    manifest {
        attributes(
            "Implementation-Title" to "core-kotlin",
            "Implementation-Version" to archiveVersion.get()
        )
    }
}

tasks.shadowJar {
    archiveBaseName.set("code-kotlin-engine")
    archiveClassifier.set("all")
    archiveVersion.set("")
    mergeServiceFiles()
}

tasks.register("printJniInfo") {
    group = "help"
    description = "Print the expected JNI library name so tooling can load it"
    doLast {
        println("JNI library expected on java.library.path as libcodex_core_jni.so")
    }
}

application {
    mainClass.set("ai.lightcode.core.engine.EngineSandboxKt")
}
