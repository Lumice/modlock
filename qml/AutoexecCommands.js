// Generated from Deadlock Mod Manager (GPL-3.0) @92bebec:
// apps/desktop/src/lib/autoexec/predefined-commands.ts and locales/en.json.
.pragma library

var categories = [
 {
  "id": "performance",
  "label": "Performance",
  "commands": [
   {
    "id": "fpsMax",
    "command": "fps_max",
    "value": "240",
    "default": false,
    "description": "Main FPS limit. 0 means unlimited."
   },
   {
    "id": "fpsMaxUi",
    "command": "fps_max_ui",
    "value": "144",
    "default": false,
    "description": "FPS limit while game UI is displayed. 0 means unlimited."
   },
   {
    "id": "engineNoFocusSleep",
    "command": "engine_no_focus_sleep",
    "value": "0",
    "default": false,
    "description": "Sleep delay when the game window is not focused. 0 keeps FPS active in background."
   },
   {
    "id": "engineLowLatencySleepAfterClientTick",
    "command": "engine_low_latency_sleep_after_client_tick",
    "value": "true",
    "default": false,
    "description": "Moves low-latency sleep after client simulation when low latency is active."
   }
  ]
 },
 {
  "id": "network",
  "label": "Network",
  "commands": [
   {
    "id": "rate",
    "command": "rate",
    "value": "1000000",
    "default": false,
    "description": "Maximum client bandwidth rate."
   },
   {
    "id": "clUpdaterate",
    "command": "cl_updaterate",
    "value": "20",
    "default": false,
    "description": "Requested server update packets per second."
   },
   {
    "id": "clInterpRatio",
    "command": "cl_interp_ratio",
    "value": "0",
    "default": false,
    "description": "Client interpolation ratio."
   },
   {
    "id": "clLagcompensation",
    "command": "cl_lagcompensation",
    "value": "true",
    "default": false,
    "description": "Keep lag compensation enabled."
   },
   {
    "id": "clHudTelemetryNetMisdeliveryShow",
    "command": "cl_hud_telemetry_net_misdelivery_show",
    "value": "1",
    "default": false,
    "description": "Show missed user commands and server snapshots only in poor conditions."
   },
   {
    "id": "clHudTelemetryNetQualityGraphShow",
    "command": "cl_hud_telemetry_net_quality_graph_show",
    "value": "0",
    "default": false,
    "description": "Hide network quality graph."
   }
  ]
 },
 {
  "id": "hudUi",
  "label": "HUD & UI",
  "commands": [
   {
    "id": "citadelUnitStatusUseNew",
    "command": "citadel_unit_status_use_new",
    "value": "true",
    "default": false,
    "description": "Enable new unit status bars."
   },
   {
    "id": "citadelHudVisible",
    "command": "citadel_hud_visible",
    "value": "true",
    "default": true,
    "description": "Default/reset: show the HUD."
   },
   {
    "id": "citadelHideReplayHud",
    "command": "citadel_hide_replay_hud",
    "value": "false",
    "default": true,
    "description": "Default/reset: keep the replay HUD visible."
   },
   {
    "id": "citadelHintSystemDisable",
    "command": "citadel_hint_system_disable",
    "value": "true",
    "default": false,
    "description": "Disable gameplay hints."
   },
   {
    "id": "citadelAlwaysShowActiveHudStats",
    "command": "citadel_always_show_active_hud_stats",
    "value": "true",
    "default": false,
    "description": "Always show active HUD stats."
   },
   {
    "id": "citadelShowActiveSlotPopup",
    "command": "citadel_show_active_slot_popup",
    "value": "true",
    "default": false,
    "description": "Show active slot popup."
   },
   {
    "id": "citadelShowAllPurchaseToasts",
    "command": "citadel_show_all_purchase_toasts",
    "value": "true",
    "default": false,
    "description": "Show all purchase toasts."
   },
   {
    "id": "citadelShopDefaultTab",
    "command": "citadel_shop_default_tab",
    "value": "-1",
    "default": true,
    "description": "Default/reset: use the game's default shop tab."
   },
   {
    "id": "clAutoCursorScale",
    "command": "cl_auto_cursor_scale",
    "value": "true",
    "default": false,
    "description": "Enable automatic cursor scaling."
   },
   {
    "id": "clShowfps",
    "command": "cl_showfps",
    "value": "0",
    "default": true,
    "description": "Default/reset: hide the FPS counter."
   },
   {
    "id": "clShowframenumber",
    "command": "cl_showframenumber",
    "value": "false",
    "default": true,
    "description": "Default/reset: hide the frame number."
   },
   {
    "id": "clTrueviewShowStatus",
    "command": "cl_trueview_show_status",
    "value": "2",
    "default": false,
    "description": "Show TrueView status. 0 never, 1 only if problem, 2 always."
   }
  ]
 },
 {
  "id": "minimap",
  "label": "Minimap",
  "commands": [
   {
    "id": "minimapUpdateRateHz",
    "command": "minimap_update_rate_hz",
    "value": "60",
    "default": true,
    "description": "Default/reset: update the minimap at 60 Hz."
   },
   {
    "id": "citadelMinimapUnitClickRadius",
    "command": "citadel_minimap_unit_click_radius",
    "value": "200",
    "default": true,
    "description": "Default/reset: restore minimap unit click radius."
   },
   {
    "id": "citadelMinimapPlayerWidth",
    "command": "citadel_minimap_player_width",
    "value": "7",
    "default": true,
    "description": "Default/reset: restore minimap player icon width."
   },
   {
    "id": "citadelMinimapLocalPlayerWidth",
    "command": "citadel_minimap_local_player_width",
    "value": "12",
    "default": true,
    "description": "Default/reset: restore local player minimap icon width."
   },
   {
    "id": "citadelMinimapMaxIconShrink",
    "command": "citadel_minimap_max_icon_shrink",
    "value": "0.8",
    "default": true,
    "description": "Default/reset: restore minimap icon shrink behavior."
   },
   {
    "id": "citadelMinimapOverlapScanDistance",
    "command": "citadel_minimap_overlap_scan_distance",
    "value": "12.5",
    "default": true,
    "description": "Default/reset: restore minimap overlap scan distance."
   },
   {
    "id": "citadelMinimapZipLineThickness",
    "command": "citadel_minimap_zip_line_thickness",
    "value": "2",
    "default": true,
    "description": "Default/reset: restore minimap zipline thickness."
   },
   {
    "id": "citadelMinimapSpectatorFowTeamView",
    "command": "citadel_minimap_spectator_fow_team_view",
    "value": "1",
    "default": true,
    "description": "Default/reset: restore spectator minimap team view."
   }
  ]
 },
 {
  "id": "matchmaking",
  "label": "Matchmaking",
  "commands": [
   {
    "id": "mmPreferSoloOnly",
    "command": "mm_prefer_solo_only",
    "value": "1",
    "default": false,
    "description": "Prefer matches with solo players."
   }
  ]
 },
 {
  "id": "privateLobbyDefaults",
  "label": "Private Lobby Defaults",
  "commands": [
   {
    "id": "citadelPrivateLobbyBotDifficulty",
    "command": "citadel_private_lobby_bot_difficulty",
    "value": "0",
    "default": true,
    "description": "Default/reset: restore private lobby bot difficulty."
   },
   {
    "id": "citadelPrivateLobbyCheatsEnabled",
    "command": "citadel_private_lobby_cheats_enabled",
    "value": "false",
    "default": true,
    "description": "Default/reset: keep private lobby cheats disabled."
   },
   {
    "id": "citadelPrivateLobbyDuplicateHeroesEnabled",
    "command": "citadel_private_lobby_duplicate_heroes_enabled",
    "value": "false",
    "default": true,
    "description": "Default/reset: keep duplicate heroes disabled."
   },
   {
    "id": "citadelPrivateLobbyIsPubliclyVisible",
    "command": "citadel_private_lobby_is_publicly_visible",
    "value": "false",
    "default": true,
    "description": "Default/reset: keep private lobbies hidden from public lists."
   },
   {
    "id": "citadelPrivateLobbyRandomizeLanes",
    "command": "citadel_private_lobby_randomize_lanes",
    "value": "false",
    "default": true,
    "description": "Default/reset: keep random lanes disabled."
   },
   {
    "id": "citadelPrivateLobbyServerRegion",
    "command": "citadel_private_lobby_server_region",
    "value": "1",
    "default": true,
    "description": "Default/reset: restore the private lobby server region."
   }
  ]
 },
 {
  "id": "mouseSensitivity",
  "label": "Mouse & Sensitivity",
  "commands": [
   {
    "id": "fovDesired",
    "command": "fov_desired",
    "value": "75",
    "default": true,
    "description": "Default/reset: restore base field of view."
   },
   {
    "id": "mYaw",
    "command": "m_yaw",
    "value": "0.022",
    "default": true,
    "description": "Default/reset: restore mouse yaw factor."
   },
   {
    "id": "mPitch",
    "command": "m_pitch",
    "value": "0.022",
    "default": true,
    "description": "Default/reset: restore mouse pitch factor."
   },
   {
    "id": "zoomSensitivityRatio",
    "command": "zoom_sensitivity_ratio",
    "value": "0.818933027098955175",
    "default": true,
    "description": "Default/reset: restore ADS / zoom sensitivity scale."
   },
   {
    "id": "clCitadelZoomIsToggle",
    "command": "cl_citadel_zoom_is_toggle",
    "value": "false",
    "default": true,
    "description": "Default/reset: use hold-to-zoom behavior."
   }
  ]
 },
 {
  "id": "abilityQuickcast",
  "label": "Ability Quickcast",
  "commands": [
   {
    "id": "clCitadelQuickcastAbility1",
    "command": "cl_citadel_quickcast_ability1",
    "value": "0",
    "default": true,
    "description": "Default/reset: restore ability 1 quickcast mode."
   },
   {
    "id": "clCitadelQuickcastAbility2",
    "command": "cl_citadel_quickcast_ability2",
    "value": "0",
    "default": true,
    "description": "Default/reset: restore ability 2 quickcast mode."
   },
   {
    "id": "clCitadelQuickcastAbility3",
    "command": "cl_citadel_quickcast_ability3",
    "value": "0",
    "default": true,
    "description": "Default/reset: restore ability 3 quickcast mode."
   },
   {
    "id": "clCitadelQuickcastAbility4",
    "command": "cl_citadel_quickcast_ability4",
    "value": "0",
    "default": true,
    "description": "Default/reset: restore ability 4 quickcast mode."
   }
  ]
 },
 {
  "id": "buildItems",
  "label": "Build & Items",
  "commands": [
   {
    "id": "citadelAutoQueueBuild",
    "command": "citadel_auto_queue_build",
    "value": "false",
    "default": false,
    "description": "Automatically queue selected build at game start."
   }
  ]
 },
 {
  "id": "audio",
  "label": "Audio",
  "commands": [
   {
    "id": "volume",
    "command": "volume",
    "value": "1",
    "default": true,
    "description": "Default/reset: restore master volume."
   },
   {
    "id": "sndGamevolume",
    "command": "snd_gamevolume",
    "value": "1",
    "default": true,
    "description": "Default/reset: restore game volume."
   },
   {
    "id": "sndGamevoicevolume",
    "command": "snd_gamevoicevolume",
    "value": "1",
    "default": true,
    "description": "Default/reset: restore game voice-over volume."
   },
   {
    "id": "sndVoipvolume",
    "command": "snd_voipvolume",
    "value": "1",
    "default": true,
    "description": "Default/reset: restore voice chat volume."
   },
   {
    "id": "soundDeviceOverride",
    "command": "sound_device_override",
    "value": "",
    "default": true,
    "description": "Default/reset: use the default sound device."
   }
  ]
 },
 {
  "id": "voiceChat",
  "label": "Voice Chat",
  "commands": [
   {
    "id": "voiceModenable",
    "command": "voice_modenable",
    "value": "true",
    "default": true,
    "description": "Default/reset: keep voice chat enabled."
   },
   {
    "id": "voiceVox",
    "command": "voice_vox",
    "value": "0",
    "default": true,
    "description": "Default/reset: use push-to-talk instead of always-on VOX."
   },
   {
    "id": "voiceThreshold",
    "command": "voice_threshold",
    "value": "-120",
    "default": false,
    "description": "Voice activation threshold."
   },
   {
    "id": "voiceLoopback",
    "command": "voice_loopback",
    "value": "false",
    "default": true,
    "description": "Default/reset: keep voice loopback disabled."
   },
   {
    "id": "voiceAlwaysSampleMic",
    "command": "voice_always_sample_mic",
    "value": "false",
    "default": true,
    "description": "Default/reset: keep voice input stream closed until needed."
   },
   {
    "id": "voiceDeviceOverride",
    "command": "voice_device_override",
    "value": "",
    "default": true,
    "description": "Default/reset: use the default voice capture device."
   }
  ]
 },
 {
  "id": "visualEffects",
  "label": "Visual / Effects",
  "commands": [
   {
    "id": "clRagdollLimit",
    "command": "cl_ragdoll_limit",
    "value": "20",
    "default": false,
    "description": "Limit visible ragdolls."
   },
   {
    "id": "violenceAblood",
    "command": "violence_ablood",
    "value": "true",
    "default": false,
    "description": "Draw alien blood."
   },
   {
    "id": "violenceAgibs",
    "command": "violence_agibs",
    "value": "true",
    "default": false,
    "description": "Show alien gibs."
   },
   {
    "id": "violenceHblood",
    "command": "violence_hblood",
    "value": "true",
    "default": false,
    "description": "Draw human blood."
   },
   {
    "id": "violenceHgibs",
    "command": "violence_hgibs",
    "value": "true",
    "default": false,
    "description": "Show human gibs."
   }
  ]
 },
 {
  "id": "replaySpectator",
  "label": "Replay / Spectator",
  "commands": [
   {
    "id": "citadelAutoHighlightSecondsBefore",
    "command": "citadel_auto_highlight_seconds_before",
    "value": "20",
    "default": true,
    "description": "Default/reset: restore replay auto-highlight lead-in seconds."
   },
   {
    "id": "citadelAutoHighlightSecondsAfter",
    "command": "citadel_auto_highlight_seconds_after",
    "value": "8",
    "default": true,
    "description": "Default/reset: restore replay auto-highlight follow-up seconds."
   },
   {
    "id": "citadelReplayManagerDownloadChunkSize",
    "command": "citadel_replay_manager_download_chunk_size",
    "value": "1048576",
    "default": true,
    "description": "Default/reset: restore replay download chunk size."
   },
   {
    "id": "citadelReplayManagerDownloadSimultaneousRequests",
    "command": "citadel_replay_manager_download_simultaneous_requests",
    "value": "3",
    "default": true,
    "description": "Default/reset: restore replay download parallel request count."
   }
  ]
 }
];
