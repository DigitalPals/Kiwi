package io.github.digitalpals.kiwi.packages

import io.github.digitalpals.kiwi.api.InstalledPackage

/** What an APK file says about itself. */
data class ArchiveInfo(
    val packageName: String,
    val versionCode: Long,
    /** SHA-256 (lowercase hex) of each signing certificate; empty when Android can't read them. */
    val signers: List<String>,
)

/**
 * What the app reads from PackageManager ([PackageScanner]). An interface, so the install flow
 * and the package report are tested on the JVM with a fake.
 */
interface PackageLookup {
    /** The given packages that are installed; the others are left out. Nothing else is read. */
    fun scan(packages: Set<String>): List<ScannedPackage>

    /** One installed package, or null when it isn't installed. */
    fun find(packageName: String): InstalledPackage?

    /** What the APK at [path] is, or null when Android can't read it. */
    fun archive(path: String): ArchiveInfo?
}
