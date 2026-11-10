package io.github.digitalpals.kiwi.packages

import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.InstalledPackage
import io.github.digitalpals.kiwi.api.PackagesReport
import io.github.digitalpals.kiwi.api.Platforms
import java.security.MessageDigest

/** What the scanner read about one package, before it goes into a report. */
data class ScannedPackage(
    val packageName: String,
    val versionName: String?,
    val versionCode: Long,
    /** DER-encoded signing certificates. */
    val certificates: List<ByteArray>,
    val installer: String?,
    /** Part of the system image (`FLAG_SYSTEM`). */
    val system: Boolean,
    /** A system app the user has updated (`FLAG_UPDATED_SYSTEM_APP`). */
    val updatedSystem: Boolean,
)

/** Builds the body of `PUT /api/me/packages` from the scanned packages. */
object PackageReport {
    /**
     * The packages Kiwi asks about, and nothing else of the phone's apps: this app ([own]), every
     * Android app Kiwi offers ([offered], `GET /api/apps?hidden=1`, hidden ones included) and
     * every install Kiwi recorded for this phone ([library], `GET /api/library`, orphaned ones
     * included). Kiwi marks a recorded install whose package the report lacks as removed, so
     * all of them must be looked up.
     */
    fun wanted(own: String, offered: List<AppSummary>, library: List<AppSummary>): Set<String> = buildSet {
        add(own)
        for (app in offered) {
            if (app.platform != Platforms.ANDROID) continue
            add(app.bundleId)
            add(app.packageName)
        }
        library.forEach { add(it.packageName) }
    }

    /**
     * The report of the [scanned] packages that are in [wanted], sorted by package, so the same
     * phone gives the same report.
     */
    fun build(scanned: List<ScannedPackage>, wanted: Set<String>): PackagesReport = PackagesReport(
        scanned.asSequence()
            .filter { it.packageName in wanted }
            .distinctBy { it.packageName }
            .sortedBy { it.packageName }
            .map(::toInstalled)
            .toList(),
    )

    fun toInstalled(scanned: ScannedPackage): InstalledPackage = InstalledPackage(
        packageName = scanned.packageName,
        // Android lets an app leave out its version name; Kiwi needs some version to show.
        versionName = scanned.versionName?.takeIf { it.isNotBlank() } ?: scanned.versionCode.toString(),
        versionCode = scanned.versionCode,
        signerSha256 = scanned.certificates.map(::sha256Hex).distinct(),
        installer = scanned.installer?.takeIf { it.isNotBlank() },
    )

    /** SHA-256 of a DER certificate, as Kiwi's Android runner computes it: lowercase hex. */
    fun sha256Hex(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    /**
     * Whether an installed package and an APK share a signer, so Android would accept the APK as
     * an update. Unknown signers (empty lists) are not treated as a conflict.
     */
    fun signersCompatible(installed: List<String>, apk: List<String>): Boolean =
        installed.isEmpty() || apk.isEmpty() || installed.any { it in apk }

    /**
     * Whether an APK's [signers] include [expected], the certificate SHA-256 Kiwi signs Android
     * apps with (`Me.host.android_signer_sha256`, hex with or without colons). Unknown signers
     * don't match.
     */
    fun signedBy(signers: List<String>, expected: String): Boolean {
        val wanted = normalizeSha256(expected)
        return signers.any { normalizeSha256(it) == wanted }
    }

    private fun normalizeSha256(hex: String): String = hex.replace(":", "").trim().lowercase()
}
