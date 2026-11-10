package io.github.digitalpals.kiwi.api

import kotlinx.serialization.builtins.ListSerializer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ModelsTest {
    private val json = KiwiJson

    @Test
    fun parsesMeFromAKiwiWithoutTheNewFields() {
        val me = json.decodeFromString(Me.serializer(), ME_BEFORE_PLATFORMS)
        assertEquals("ts:nTEST1CNTRL", me.device.id)
        assertEquals("android", me.device.platform)
        assertNull(me.device.client)
        assertNull(me.onboarded)
        assertNull(me.androidApp)
        assertTrue(me.features.isEmpty())
        assertEquals(2, me.counts.updates)
        assertEquals("https://mac.example.ts.net", me.server.publicUrl)
    }

    @Test
    fun parsesMeWithPlatformFeaturesAndTheAndroidApp() {
        val me = json.decodeFromString(Me.serializer(), ME_WITH_PLATFORMS)
        assertEquals("detected", me.device.platformSource)
        assertEquals("android", me.device.detectedPlatform)
        assertEquals("android_app", me.device.client)
        assertEquals(false, me.onboarded)
        assertEquals(listOf("android_apps", "web_push"), me.features)
        assertEquals("linux", me.host?.os)
        assertEquals("0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0", me.host?.androidSignerSha256)
        // Before Kiwi Builds loaded its key (and on older servers) there is none.
        assertNull(json.decodeFromString(HostInfo.serializer(), """{"os":"macos","android_signer_sha256":null}""").androidSignerSha256)
        assertNull(json.decodeFromString(HostInfo.serializer(), """{"os":"macos"}""").androidSignerSha256)
        val app = me.androidApp!!
        assertEquals("io.github.digitalpals.kiwi", app.packageName)
        assertEquals(3L, app.versionCode)
        assertEquals("https://mac.example.ts.net/android/kiwi.apk", app.installUrl)
    }

    @Test
    fun parsesAnAndroidAppSummaryWithItsState() {
        val apps = json.decodeFromString(ListSerializer(AppSummary.serializer()), "[$ANDROID_APP]")
        val app = apps.single()
        assertEquals("android", app.platform)
        assertEquals("app.morphe.android.youtube", app.packageName)
        assertEquals("kiwi-youtube/app.morphe.android.youtube", app.key)
        assertEquals(Action.Update, app.state.action)
        assertEquals("21.44.0", app.state.target?.version)
        assertEquals(3, app.state.target?.revision)
        assertEquals("https://mac.example.ts.net/ota/tok123/app.apk", app.state.installUrl)
        assertTrue(app.state.foreignInstall)
        assertNull(app.state.build?.expiresAt)
        assertTrue(app.state.build!!.isReady)
    }

    @Test
    fun unknownActionsAndNullsFallBack() {
        val state = json.decodeFromString(
            AppState.serializer(),
            """{"action":"teleport","update_reason":null,"installed":null,"target":null,"build":null,"install_url":null,"open_url":null,"replaces":null,"foreign_install":null}""",
        )
        assertEquals(Action.Unknown, state.action)
        assertFalse(state.foreignInstall)
    }

    @Test
    fun parsesAnAppDetail() {
        val detail = json.decodeFromString(AppDetail.serializer(), APP_DETAIL)
        assertEquals("Patched YouTube", detail.description)
        assertEquals(2, detail.releases.size)
        assertEquals("app.morphe.android.youtube", detail.signedBundleId)
        assertEquals(Action.Install, detail.summary.state.action)
        assertEquals(detail.name, detail.summary.name)
    }

    @Test
    fun parsesBuildsAndSettings() {
        val build = json.decodeFromString(
            Build.serializer(),
            """{"id":"b1","source_id":"s","bundle_id":"x","version":"1.0","build":null,"status":"downloading","step":"Downloading","progress":0.5,"error":null,"signed_bundle_id":"x","size":10,"expires_at":null,"dropped_entitlements":[],"created_at":"2026-11-10T10:00:00Z","finished_at":null,"install_url":null}""",
        )
        assertTrue(build.isActive)
        assertEquals(0.5, build.progress!!, 0.0)
        val settings = json.decodeFromString(
            Settings.serializer(),
            """{"refresh_interval_minutes":30,"auto_prepare_updates":false,"notify_updates":true,"notify_expiry":true,"devicectl_sync":true,"devicectl_sync_minutes":10,"strip_watch_apps":true,"app_store_country":"nl","kiwi_builds":true}""",
        )
        assertEquals(30, settings.refreshIntervalMinutes)
        assertTrue(settings.kiwiBuilds)
        assertEquals("nl", settings.appStoreCountry)
    }

    @Test
    fun encodesThePackagesReportInTheContractShape() {
        val report = PackagesReport(
            listOf(InstalledPackage("com.example.app", "1.2", 12, listOf("ab", "cd"), null)),
        )
        assertEquals(
            """{"packages":[{"package":"com.example.app","version_name":"1.2","version_code":12,"signer_sha256":["ab","cd"],"installer":null}]}""",
            json.encodeToString(PackagesReport.serializer(), report),
        )
    }

    @Test
    fun parsesErrorBodies() {
        val body = json.decodeFromString(
            ErrorBody.serializer(),
            """{"error":{"code":"not_owner","message":"This Kiwi belongs to alex@example.com."}}""",
        )
        assertEquals("not_owner", body.error.code)
    }

    companion object {
        const val ME_BEFORE_PLATFORMS = """
        {"device":{"id":"ts:nTEST1CNTRL","name":"Pixel","kind":"tailscale","tailscale_name":"pixel","platform":"android",
          "os_version":null,"os_version_source":null,"model":"Android","devicectl":null,"last_seen":"2026-11-10T10:00:00Z"},
         "server":{"version":"0.9.0","public_url":"https://mac.example.ts.net","team_id":null,"devicectl_available":false},
         "push":{"vapid_public_key":"BOk","subscribed":false},
         "counts":{"updates":2,"installed":3,"sources":1,"source_errors":0,"preparing":0},
         "warnings":[]}
        """

        const val ME_WITH_PLATFORMS = """
        {"device":{"id":"ts:nTEST1CNTRL","name":"Pixel","kind":"tailscale","tailscale_name":"pixel","platform":"android",
          "platform_source":"detected","detected_platform":"android","client":"android_app",
          "os_version":"16","os_version_source":"android_app","model":"Android","devicectl":null,"last_seen":"2026-11-10T10:00:00Z"},
         "server":{"version":"1.0.0","public_url":"https://mac.example.ts.net","team_id":null,"devicectl_available":false},
         "push":{"vapid_public_key":"BOk","subscribed":false},
         "counts":{"updates":0,"installed":0,"sources":1,"source_errors":0,"preparing":0},
         "warnings":[{"level":"info","code":"x","message":"m","link":null}],
         "onboarded":false,
         "host":{"os":"linux","ios_signing":"files","devicectl":false,"android_signer_sha256":"0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0"},
         "features":["android_apps","web_push"],
         "android_app":{"package":"io.github.digitalpals.kiwi","version_name":"0.3.0","version_code":3,"size":1234,
           "install_url":"https://mac.example.ts.net/android/kiwi.apk"}}
        """

        const val STATE = """
        {"action":"update","update_reason":"new_version",
         "installed":{"version":"21.43.5","build":"1550000000","revision":2,"signed_bundle_id":"app.morphe.android.youtube",
           "origin":"kiwi","verified":true,"installed_at":"2026-11-01T10:00:00Z","expires_at":null},
         "target":{"version":"21.44.0","build":"1551000000","revision":3,"date":"2026-11-09","notes":null,"size":150000000,
           "min_os":null,"max_os":null,"compatible":true,"incompatibility":null},
         "build":{"id":"b2","source_id":"kiwi-youtube","bundle_id":"app.morphe.android.youtube","version":"21.44.0",
           "build":"1551000000","revision":3,"status":"ready","step":null,"progress":null,"error":null,
           "signed_bundle_id":"app.morphe.android.youtube","size":150000000,"expires_at":null,"dropped_entitlements":[],
           "created_at":"2026-11-09T10:00:00Z","finished_at":"2026-11-09T10:00:00Z",
           "install_url":"https://mac.example.ts.net/ota/tok123/app.apk"},
         "install_url":"https://mac.example.ts.net/ota/tok123/app.apk","open_url":null,"replaces":null,"foreign_install":true}
        """

        const val ANDROID_APP = """
        {"source_id":"kiwi-youtube","source_name":"Kiwi Builds","bundle_id":"app.morphe.android.youtube","platform":"android",
         "name":"YouTube","developer":"Google","subtitle":"Morphe","icon_url":"https://is1-ssl.mzstatic.com/x.png",
         "tint_color":"#ff0000","category":null,"unavailable":null,"orphaned":false,"hidden":false,"is_new":false,
         "original":null,"state":$STATE}
        """

        const val APP_DETAIL = """
        {"source_id":"kiwi-youtube","source_name":"Kiwi Builds","bundle_id":"app.morphe.android.youtube","platform":"android",
         "name":"YouTube","developer":null,"subtitle":null,"icon_url":null,"tint_color":null,"category":null,
         "unavailable":null,"orphaned":false,"hidden":false,"is_new":false,"original":null,
         "state":{"action":"install","update_reason":null,"installed":null,"target":null,"build":null,
           "install_url":"https://mac.example.ts.net/ota/t/app.apk","open_url":null,"replaces":null},
         "description":"Patched YouTube","screenshots":[],
         "releases":[{"version":"2","build":null,"date":null,"notes":null,"size":null,"min_os":null,"max_os":null,"compatible":true,"incompatibility":null},
                     {"version":"1","build":null,"date":null,"notes":null,"size":null,"min_os":null,"max_os":null,"compatible":true,"incompatibility":null}],
         "permissions":{"entitlements":[],"privacy":[]},"dropped_entitlements":[],
         "signed_bundle_id":"app.morphe.android.youtube","source_url":"kiwi://local-build","original_match":"none","other_versions":[]}
        """
    }

    @Test
    fun parsesTheKiwiBuildsOverview() {
        val text = """
            {"apps":[{"id":"android-youtube","platform":"android","name":"YouTube","bundle_id":"app.morphe.android.youtube",
              "icon_url":null,"tweak":"Morphe Patches 2.1.0","summary":"No ads","experimental":false,"warning":null,"names":[],
              "app_version":"21.30.216","limitations":[{"icon":"key","title":"Your key","detail":"Signed by Kiwi"}],
              "included":[{"id":"ads","name":"Remove ads","description":"No ads"}],
              "available_tweaks":[{"id":"sb","name":"SponsorBlock","description":"Skips sponsors"}],
              "added":true,"preferences":{"enabled":true,"interval_minutes":360,"tweaks":["sb"]},
              "jobs":[{"id":"j1","app":"android-youtube","sequence":4,"status":"running","stage":"Patching","version":"21.31.0",
                "external_id":"1561187872","recipe_key":null,"signed_build_id":null,"error":null,
                "created_at":"2026-11-10T08:00:00Z","finished_at":null}],
              "checks":[],"last_checked_at":"2026-11-10T08:00:00Z","last_success_key":null,"failed_recipe":null,
              "failed_attempts":0,"sequence":4,"published":{"version":"21.30.216","build":null,"revision":null,
              "date":"2026-11-09T10:00:00Z","tweaks":["sb"],"name":null},"source_enabled":true,
              "configuration_changed":false,"next_check_at":null}],
             "checks":[{"name":"macOS","ready":false}]}
        """.trimIndent()
        val overview = KiwiJson.decodeFromString(AutomationOverview.serializer(), text)
        val app = overview.apps.single()
        assertEquals("app.morphe.android.youtube", app.bundleId)
        assertEquals(listOf("sb"), app.preferences.tweaks)
        assertEquals(360, app.preferences.intervalMinutes)
        assertEquals("Patching", app.jobs.single().stage)
        assertTrue(app.jobs.single().isActive)
        assertEquals(listOf("sb"), app.published?.tweaks)
        assertEquals("SponsorBlock", app.availableTweaks.single().name)
        assertEquals(false, overview.checks.single().ready)
    }
}
