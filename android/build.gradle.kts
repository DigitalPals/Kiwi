// AGP 9 compiles Kotlin itself ("built-in Kotlin") and brings an older Kotlin Gradle plugin;
// putting the catalog's version on the classpath makes the compiler match the Compose and
// serialization plugins.
buildscript {
    dependencies {
        classpath(libs.kotlin.gradle.plugin)
    }
}

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.compose) apply false
    alias(libs.plugins.kotlin.serialization) apply false
}
