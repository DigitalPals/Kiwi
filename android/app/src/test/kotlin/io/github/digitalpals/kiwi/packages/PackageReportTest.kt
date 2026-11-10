package io.github.digitalpals.kiwi.packages

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
    fun reportsUserInstalledAndUpdatedSystemPackagesSorted() {
        val report = PackageReport.build(
            listOf(
                scanned("com.reddit.frontpage"),
                scanned("com.android.systemui", system = true),
                scanned("com.google.android.youtube", system = true, updated = true),
                scanned("app.morphe.android.youtube"),
                scanned("app.morphe.android.youtube"),
            ),
        )
        assertEquals(
            listOf("app.morphe.android.youtube", "com.google.android.youtube", "com.reddit.frontpage"),
            report.packages.map { it.packageName },
        )
    }

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
        val report = PackageReport.build(listOf(scanned("com.example.app", certs = listOf("abc".toByteArray()), installer = null)))
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
