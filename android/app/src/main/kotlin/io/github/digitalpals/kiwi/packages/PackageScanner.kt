package io.github.digitalpals.kiwi.packages

import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.content.pm.Signature
import android.os.Build
import io.github.digitalpals.kiwi.api.InstalledPackage

/**
 * Reads the installed packages with PackageManager. The app holds `QUERY_ALL_PACKAGES` (it is
 * not distributed through Google Play), so it sees every package, not only those it declares.
 */
class PackageScanner(context: Context) {
    private val context = context.applicationContext
    private val pm: PackageManager = this.context.packageManager

    /** Every installed package with its version, signers and installer. */
    fun scan(): List<ScannedPackage> = installedPackages().map(::toScanned)

    /** One installed package, or null when it isn't installed. */
    fun find(packageName: String): InstalledPackage? =
        packageInfo(packageName)?.let { PackageReport.toInstalled(toScanned(it)) }

    fun isInstalled(packageName: String): Boolean = packageInfo(packageName) != null

    /** The signers of an APK file (lowercase SHA-256 hex), or null when Android can't read them. */
    fun archiveSigners(path: String): ArchiveInfo? {
        val info = if (Build.VERSION.SDK_INT >= 33) {
            pm.getPackageArchiveInfo(path, PackageManager.PackageInfoFlags.of(SIGNATURE_FLAGS.toLong()))
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageArchiveInfo(path, SIGNATURE_FLAGS)
        } ?: return null
        return ArchiveInfo(
            packageName = info.packageName,
            versionCode = versionCode(info),
            signers = certificates(info).map { PackageReport.sha256Hex(it.toByteArray()) }.distinct(),
        )
    }

    /**
     * Whether Android lets this app update [packageName] without asking (API 31+): this app must
     * be the package's installer of record, or its update owner (API 34+).
     */
    fun canUpdateSilently(packageName: String): Boolean {
        if (Build.VERSION.SDK_INT < 31) return false
        val source = runCatching { pm.getInstallSourceInfo(packageName) }.getOrNull() ?: return false
        val own = context.packageName
        if (source.installingPackageName == own) return true
        return Build.VERSION.SDK_INT >= 34 && source.updateOwnerPackageName == own
    }

    private fun installedPackages(): List<PackageInfo> = if (Build.VERSION.SDK_INT >= 33) {
        pm.getInstalledPackages(PackageManager.PackageInfoFlags.of(SIGNATURE_FLAGS.toLong()))
    } else {
        @Suppress("DEPRECATION")
        pm.getInstalledPackages(SIGNATURE_FLAGS)
    }

    private fun packageInfo(packageName: String): PackageInfo? = try {
        if (Build.VERSION.SDK_INT >= 33) {
            pm.getPackageInfo(packageName, PackageManager.PackageInfoFlags.of(SIGNATURE_FLAGS.toLong()))
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageInfo(packageName, SIGNATURE_FLAGS)
        }
    } catch (_: PackageManager.NameNotFoundException) {
        null
    }

    private fun toScanned(info: PackageInfo): ScannedPackage {
        val flags = info.applicationInfo?.flags ?: 0
        return ScannedPackage(
            packageName = info.packageName,
            versionName = info.versionName,
            versionCode = versionCode(info),
            certificates = certificates(info).map(Signature::toByteArray),
            installer = installer(info.packageName),
            system = flags and ApplicationInfo.FLAG_SYSTEM != 0,
            updatedSystem = flags and ApplicationInfo.FLAG_UPDATED_SYSTEM_APP != 0,
        )
    }

    private fun versionCode(info: PackageInfo): Long =
        if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else @Suppress("DEPRECATION") info.versionCode.toLong()

    /**
     * The package's signing certificates: all signers of a multi-signer APK, else the signer
     * with its rotation history (Kiwi matches any of them against its own certificate).
     */
    private fun certificates(info: PackageInfo): List<Signature> {
        if (Build.VERSION.SDK_INT >= 28) {
            val signing = info.signingInfo ?: return emptyList()
            val signatures = if (signing.hasMultipleSigners()) signing.apkContentsSigners else signing.signingCertificateHistory
            return signatures?.toList().orEmpty()
        }
        @Suppress("DEPRECATION")
        return info.signatures?.toList().orEmpty()
    }

    private fun installer(packageName: String): String? = try {
        if (Build.VERSION.SDK_INT >= 30) {
            pm.getInstallSourceInfo(packageName).installingPackageName
        } else {
            @Suppress("DEPRECATION")
            pm.getInstallerPackageName(packageName)
        }
    } catch (_: PackageManager.NameNotFoundException) {
        null
    } catch (_: IllegalArgumentException) {
        null
    }

    data class ArchiveInfo(val packageName: String, val versionCode: Long, val signers: List<String>)

    private companion object {
        @Suppress("DEPRECATION")
        val SIGNATURE_FLAGS: Int =
            if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
    }
}
