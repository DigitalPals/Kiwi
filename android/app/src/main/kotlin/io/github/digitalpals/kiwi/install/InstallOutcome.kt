package io.github.digitalpals.kiwi.install

import android.content.pm.PackageInstaller

/** How a PackageInstaller session ended, in words for the user. */
sealed interface InstallOutcome {
    data object Success : InstallOutcome

    /** The user closed Android's confirmation. */
    data object Cancelled : InstallOutcome

    /** The package is installed with another signature: the user must remove it first. */
    data class SignatureConflict(val message: String) : InstallOutcome

    data class Failure(val message: String) : InstallOutcome

    companion object {
        /**
         * Maps a session's final `EXTRA_STATUS` and `EXTRA_STATUS_MESSAGE` to an outcome.
         * Android reports a signature mismatch (`INSTALL_FAILED_UPDATE_INCOMPATIBLE`) as a
         * conflict; its status message names the cause.
         */
        fun from(status: Int, statusMessage: String?, appName: String): InstallOutcome {
            val detail = statusMessage?.trim()?.takeIf { it.isNotEmpty() }
            return when (status) {
                PackageInstaller.STATUS_SUCCESS -> Success
                PackageInstaller.STATUS_FAILURE_ABORTED -> Cancelled
                PackageInstaller.STATUS_FAILURE_CONFLICT ->
                    if (isSignatureMismatch(detail)) {
                        SignatureConflict(signatureConflictMessage(appName))
                    } else {
                        Failure(withDetail("$appName conflicts with an app that is already installed.", detail))
                    }
                PackageInstaller.STATUS_FAILURE_INCOMPATIBLE ->
                    if (detail?.contains("VERSION_DOWNGRADE") == true) {
                        Failure("A newer version of $appName is already installed.")
                    } else {
                        Failure(withDetail("$appName isn't compatible with this phone.", detail))
                    }
                PackageInstaller.STATUS_FAILURE_STORAGE ->
                    Failure("There isn't enough storage to install $appName.")
                PackageInstaller.STATUS_FAILURE_INVALID ->
                    Failure(withDetail("The downloaded $appName isn't a valid app.", detail))
                PackageInstaller.STATUS_FAILURE_BLOCKED ->
                    Failure(withDetail("Installing $appName was blocked on this phone.", detail))
                else -> Failure(withDetail("Couldn't install $appName.", detail))
            }
        }

        fun signatureConflictMessage(appName: String): String =
            "Another $appName is installed with a different signature. Remove it first, then install again."

        private fun isSignatureMismatch(detail: String?): Boolean {
            val text = detail ?: return false
            return text.contains("UPDATE_INCOMPATIBLE") ||
                text.contains("signatures do not match", ignoreCase = true) ||
                text.contains("signature", ignoreCase = true)
        }

        private fun withDetail(message: String, detail: String?): String =
            if (detail == null) message else "$message ($detail)"
    }
}
