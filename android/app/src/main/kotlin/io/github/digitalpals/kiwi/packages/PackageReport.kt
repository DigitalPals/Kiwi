package io.github.digitalpals.kiwi.packages

import io.github.digitalpals.kiwi.api.InstalledPackage
import io.github.digitalpals.kiwi.api.PackagesReport
import java.security.MessageDigest

/** What the scanner read about one package, before it is filtered into a report. */
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
     * The packages the user installed: everything outside the system image, plus system apps
     * that were updated (an OEM may preinstall an app Kiwi also ships, and then the user's
     * copy is an update of it). Sorted by package, so the same phone gives the same report.
     */
    fun build(scanned: List<ScannedPackage>): PackagesReport = PackagesReport(
        scanned.asSequence()
            .filter { !it.system || it.updatedSystem }
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
}
