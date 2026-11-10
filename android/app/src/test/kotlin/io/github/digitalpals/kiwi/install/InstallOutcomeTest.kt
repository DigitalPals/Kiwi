package io.github.digitalpals.kiwi.install

import android.content.pm.PackageInstaller
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class InstallOutcomeTest {
    @Test
    fun mapsSessionStatuses() {
        assertEquals(InstallOutcome.Success, InstallOutcome.from(PackageInstaller.STATUS_SUCCESS, null, "YouTube"))
        assertEquals(InstallOutcome.Cancelled, InstallOutcome.from(PackageInstaller.STATUS_FAILURE_ABORTED, "User rejected", "YouTube"))
        assertEquals(
            InstallOutcome.Failure("There isn't enough storage to install YouTube."),
            InstallOutcome.from(PackageInstaller.STATUS_FAILURE_STORAGE, null, "YouTube"),
        )
    }

    @Test
    fun recognisesAnotherSignature() {
        val outcome = InstallOutcome.from(
            PackageInstaller.STATUS_FAILURE_CONFLICT,
            "INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package com.reddit.frontpage signatures do not match newer version; ignoring!",
            "Reddit",
        )
        assertEquals(
            InstallOutcome.SignatureConflict("Another Reddit is installed with a different signature. Remove it first, then install again."),
            outcome,
        )
    }

    @Test
    fun keepsAndroidsDetailForOtherFailures() {
        val conflict = InstallOutcome.from(PackageInstaller.STATUS_FAILURE_CONFLICT, "INSTALL_FAILED_CONFLICTING_PROVIDER", "Reddit")
        assertEquals(
            InstallOutcome.Failure("Reddit conflicts with an app that is already installed. (INSTALL_FAILED_CONFLICTING_PROVIDER)"),
            conflict,
        )
        val downgrade = InstallOutcome.from(PackageInstaller.STATUS_FAILURE_INCOMPATIBLE, "INSTALL_FAILED_VERSION_DOWNGRADE", "Reddit")
        assertEquals(InstallOutcome.Failure("A newer version of Reddit is already installed."), downgrade)
        val other = InstallOutcome.from(PackageInstaller.STATUS_FAILURE, null, "Reddit")
        assertTrue(other is InstallOutcome.Failure)
    }
}
