plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

// The app's version. The release workflow passes it as Gradle properties derived from the
// release tag (`-PkiwiVersionName=1.2.3 -PkiwiVersionCode=1002003`); local builds use the
// defaults, which every release outranks.
val kiwiVersionName: String = providers.gradleProperty("kiwiVersionName").orNull ?: "0.1.0"
val kiwiVersionCode: Int = providers.gradleProperty("kiwiVersionCode").orNull?.let { code ->
    code.toIntOrNull()?.takeIf { it in 1..2_100_000_000 }
        ?: throw GradleException("kiwiVersionCode must be a whole number from 1 to 2100000000, not \"$code\".")
} ?: 1
if (!Regex("[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}").matches(kiwiVersionName)) {
    throw GradleException("kiwiVersionName must be a version like 1.2.3, not \"$kiwiVersionName\".")
}

android {
    namespace = "io.github.digitalpals.kiwi"
    compileSdk = 37

    defaultConfig {
        applicationId = "io.github.digitalpals.kiwi"
        minSdk = 26
        targetSdk = 37
        versionCode = kiwiVersionCode
        versionName = kiwiVersionName
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // No signing config: release APKs are built unsigned and signed by whoever
            // distributes them (see android/README.md). Debug builds use the debug key.
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    testOptions {
        // JVM unit tests only: Android calls that are not mocked return defaults.
        unitTests.isReturnDefaultValues = true
    }

    packaging {
        resources.excludes += setOf("META-INF/{AL2.0,LGPL2.1}", "META-INF/versions/9/OSGI-INF/MANIFEST.MF")
    }

    dependenciesInfo {
        // The dependency block is for Google Play; this app is not distributed there.
        includeInApk = false
        includeInBundle = false
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.work.runtime)
    implementation(libs.androidx.datastore.preferences)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons.core)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.okhttp)
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.okhttp.mockwebserver)
}
