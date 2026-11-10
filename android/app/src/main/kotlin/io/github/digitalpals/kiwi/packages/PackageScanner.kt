package io.github.digitalpals.kiwi.packages

import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.content.pm.Signature
import android.os.Build
import io.github.digitalpals.kiwi.api.InstalledPackage

/**
 * Reads packages with PackageManager. The app holds `QUERY_ALL_PACKAGES` (it is not
 * distributed through Google Play) because the packages Kiwi asks about change with Kiwi's
 * catalogue; it only ever looks up those packages by name ([scan]) and never lists the phone's
 * other apps.
 */
class PackageScanner(context: Context) : PackageLookup {
    private val context = context.applicationContext
    private val pm: PackageManager = this.context.packageManager

    override fun scan(packages: Set<String>): List<ScannedPackage> =
        packages.sorted().mapNotNull { packageInfo(it)?.let(::toScanned) }

    override fun find(packageName: String): InstalledPackage? =
        packageInfo(packageName)?.let { PackageReport.toInstalled(toScanned(it)) }

    fun isInstalled(packageName: String): Boolean = packageInfo(packageName) != null

    /** The package, version and signers (lowercase SHA-256 hex) of an APK file. */
    override fun archive(path: String): ArchiveInfo? {
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
     * Whether Android lets this app update [packageName] without asking (API 31+). It holds
     * `UPDATE_PACKAGES_WITHOUT_USER_ACTION` and asks for `USER_ACTION_NOT_REQUIRED`; Android
     * (PackageInstallerSession's user action rules) then installs silently when:
     *
     * - the package has an update owner (API 34+, `setRequestUpdateOwnership`): this app is it.
     *   Another owner always gets a confirmation;
     * - else this app is the package's installer of record, or the package is this app itself
     *   (a self-update, whoever installed it first, e.g. the browser).
     *
     * Android also wants the APK to target a recent SDK (Kiwi targets the newest; patched apps
     * normally do) and asks anyway for a repeated update of one package within a short time.
     * Whatever Android decides, an install that needs a confirmation still shows one (or a
     * notification), so this only decides what the background worker tries.
     */
    fun canUpdateSilently(packageName: String): Boolean {
        if (Build.VERSION.SDK_INT < 31) return false
        val source = runCatching { pm.getInstallSourceInfo(packageName) }.getOrNull() ?: return false
        val owner = if (Build.VERSION.SDK_INT >= 34) source.updateOwnerPackageName else null
        return SilentUpdates.allowed(context.packageName, packageName, source.installingPackageName, owner)
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

    private companion object {
        @Suppress("DEPRECATION")
        private val SIGNATURE_FLAGS: Int =
            if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
    }
}

/** The rule of [PackageScanner.canUpdateSilently], free of Android so it is tested on the JVM. */
object SilentUpdates {
    /**
     * Whether [own] may update [packageName] without a confirmation: with an update owner, only
     * the owner; otherwise the installer of record, or the app updating itself.
     */
    fun allowed(own: String, packageName: String, installer: String?, updateOwner: String?): Boolean =
        if (updateOwner != null) updateOwner == own else installer == own || packageName == own
}
