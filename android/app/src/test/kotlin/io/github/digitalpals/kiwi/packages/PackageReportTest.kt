package io.github.digitalpals.kiwi.packages

import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Installed
import io.github.digitalpals.kiwi.api.KiwiJson
import io.github.digitalpals.kiwi.api.PackagesReport
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PackageReportTest {
    private fun scanned(
        name: String,
        system: Boolean = false,
        updated: Boolean = false,
        versionName: String? = "1.0",
        certs: List<ByteArray> = listOf("cert-$name".toByteArray()),
        installer: String? = "io.github.digitalpals.kiwi",
    ) = ScannedPackage(name, versionName, 10, certs, installer, system, updated)

    @Test
    fun reportsOnlyTheWantedPackagesSorted() {
        val report = PackageReport.build(
            listOf(
                scanned("com.reddit.frontpage"),
                scanned("com.android.systemui", system = true),
                scanned("com.example.bank"),
                scanned("io.github.digitalpals.kiwi"),
                scanned("app.morphe.android.youtube"),
                scanned("app.morphe.android.youtube"),
            ),
            wanted = setOf("app.morphe.android.youtube", "com.reddit.frontpage", "io.github.digitalpals.kiwi", "com.not.installed"),
        )
        assertEquals(
            listOf("app.morphe.android.youtube", "com.reddit.frontpage", "io.github.digitalpals.kiwi"),
            report.packages.map { it.packageName },
        )
    }

    @Test
    fun wantsKiwisOwnOfferedAndRecordedPackages() {
        val offered = listOf(
            app("kiwi-youtube", "app.morphe.android.youtube", "android"),
            // A hidden Android app still counts; Kiwi offers it by its page.
            app("kiwi-youtube", "com.reddit.frontpage", "android", hidden = true),
            // iOS apps (listed with hidden=1) are not Android packages, even when the ids look alike.
            app("altstore", "com.example.ios", "ios"),
        )
        val library = listOf(
            // An orphaned install whose source is gone: Kiwi marks it removed if it isn't reported.
            app("gone", "com.example.orphan", "android", installed = "com.example.orphan"),
        )
        assertEquals(
            setOf("io.github.digitalpals.kiwi", "app.morphe.android.youtube", "com.reddit.frontpage", "com.example.orphan"),
            PackageReport.wanted("io.github.digitalpals.kiwi", offered, library),
        )
    }

    @Test
    fun matchesKiwisSigner() {
        val signer = "ab".repeat(32)
        assertTrue(PackageReport.signedBy(listOf("cd", signer), signer))
        assertTrue(PackageReport.signedBy(listOf(signer), signer.uppercase().chunked(2).joinToString(":")))
        assertFalse(PackageReport.signedBy(listOf("cd"), signer))
        assertFalse(PackageReport.signedBy(emptyList(), signer))
    }

    private fun app(source: String, bundle: String, platform: String, hidden: Boolean = false, installed: String? = null) =
        AppSummary(
            sourceId = source,
            bundleId = bundle,
            platform = platform,
            name = bundle,
            hidden = hidden,
            state = AppState(installed = installed?.let { Installed(version = "1", signedBundleId = it) }),
        )

    @Test
    fun hashesCertificatesLikeTheAndroidRunner() {
        // SHA-256 of "abc", the standard test vector.
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            PackageReport.sha256Hex("abc".toByteArray()),
        )
        val installed = PackageReport.toInstalled(scanned("x", certs = listOf("abc".toByteArray(), "abc".toByteArray())))
        assertEquals(listOf("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"), installed.signerSha256)
    }

    @Test
    fun fillsInMissingVersionNamesAndInstallers() {
        val installed = PackageReport.toInstalled(scanned("x", versionName = null, installer = " "))
        assertEquals("10", installed.versionName)
        assertEquals(null, installed.installer)
    }

    @Test
    fun encodesTheContractBody() {
        val report = PackageReport.build(listOf(scanned("com.example.app", certs = listOf("abc".toByteArray()), installer = null)), setOf("com.example.app"))
        assertEquals(
            """{"packages":[{"package":"com.example.app","version_name":"1.0","version_code":10,""" +
                """"signer_sha256":["ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"],"installer":null}]}""",
            KiwiJson.encodeToString(PackagesReport.serializer(), report),
        )
    }

    @Test
    fun comparesSigners() {
        assertTrue(PackageReport.signersCompatible(listOf("a", "b"), listOf("b")))
        assertFalse(PackageReport.signersCompatible(listOf("a"), listOf("b")))
        assertTrue(PackageReport.signersCompatible(emptyList(), listOf("b")))
        assertTrue(PackageReport.signersCompatible(listOf("a"), emptyList()))
    }
}
