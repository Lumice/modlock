cmake_minimum_required(VERSION 3.24)
project(Modlock VERSION 0.1 LANGUAGES CXX)

find_package(Qt6 6.10 REQUIRED COMPONENTS Core Gui GuiPrivate Qml Quick Network Multimedia)
qt_standard_project_setup(REQUIRES 6.10)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_program(CARGO_EXECUTABLE cargo REQUIRED)
if(DEFINED ENV{CARGO_TARGET_DIR})
    set(HELPER_TARGET_DIR "$ENV{CARGO_TARGET_DIR}")
else()
    set(HELPER_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/helper-target")
endif()
set(HELPER_BINARY "${CMAKE_CURRENT_BINARY_DIR}/modlock-helper")
file(GLOB_RECURSE HELPER_SOURCES CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/backend/src/*.rs"
    "${CMAKE_CURRENT_SOURCE_DIR}/backend/vendor/*/src/*.rs"
    "${CMAKE_CURRENT_SOURCE_DIR}/backend/vendor/*/build.rs"
    "${CMAKE_CURRENT_SOURCE_DIR}/backend/vendor/*/Cargo.toml")
add_custom_command(OUTPUT "${HELPER_BINARY}"
    COMMAND "${CARGO_EXECUTABLE}" build --offline --locked --release
        --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/backend/Cargo.toml"
        --target-dir "${HELPER_TARGET_DIR}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${HELPER_TARGET_DIR}/release/modlock-helper" "${HELPER_BINARY}"
    DEPENDS ${HELPER_SOURCES} backend/data/perf-catalog.json backend/data/hero-mapping.json backend/Cargo.toml backend/Cargo.lock backend/vendor/json/Cargo.toml
    # Cargo discovers .cargo/config.toml from the working directory, not the
    # manifest path; run inside the repo so the vendored source replacement
    # applies even for out-of-tree build dirs (e.g. ~/.cache/...).
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/backend"
    COMMENT "Building offline Rust helper" VERBATIM)
add_custom_target(modlock-helper-build DEPENDS "${HELPER_BINARY}")

qt_add_executable(modlock-network src/networktransport/main.cpp
    src/networktransport/networktransport.h src/networktransport/networktransport.cpp
    src/networktransport/remotetransfer.h src/networktransport/remotetransfer.cpp
    src/networktransport/remoteworker.h src/networktransport/remoteworker.cpp
    src/networktransport/remoteuri.h src/networktransport/remoteuri.cpp)
target_link_libraries(modlock-network PRIVATE Qt6::Core Qt6::Network)
set_target_properties(modlock-network PROPERTIES RUNTIME_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/$<0:>")

add_subdirectory(style)

qt_add_executable(modlock src/installlink.cpp src/instancebroker.cpp src/instancebroker.h src/networktransport/remoteuri.cpp src/main.cpp src/appimagepalette.cpp src/appimagepalette.h)
set_target_properties(modlock PROPERTIES RUNTIME_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/$<0:>")
add_dependencies(modlock modlock-helper-build modlock-network)
set_source_files_properties(qml/Theme.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)
set_source_files_properties(qml/Preferences.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)
qt_add_qml_module(modlock
    URI Deadlock.Native
    VERSION 1.0
    NO_IMPORT_SCAN
    QML_FILES
        qml/FontDialog.qml qml/InstalledFontsDialog.qml qml/FontPresentation.js
        qml/LocalImportDialog.qml qml/LocalImportJob.qml
        qml/OptionsDialog.qml qml/OrderDialog.qml qml/ProfileEditReview.qml qml/PreparationHistory.qml
        qml/NativeWorkflows.qml
        qml/RemoteInstallDialog.qml qml/DownloadHistory.qml qml/DownloadBubbles.qml
        qml/CrashDialog.qml qml/LaunchWorkflows.qml qml/StatusNotice.qml qml/LaunchNotice.qml qml/LaunchDetails.qml
        qml/LaunchReview.qml qml/LaunchSummary.qml qml/LaunchSettings.qml qml/LaunchPresentation.js
        qml/NativeReviewDialog.qml
        qml/MaintenanceDialog.qml
        qml/StorageDialog.qml
        qml/ProfilesDialog.qml qml/ProfileShareDialog.qml
        qml/Main.qml qml/ControlGallery.qml
        qml/Theme.qml qml/Preferences.qml
        qml/AppHeader.qml
        qml/AppNavigation.qml qml/HomePage.qml qml/HomeModTile.qml qml/Marquee.qml qml/SetupTile.qml qml/EmptyState.qml qml/HomeState.js qml/BrowsePage.qml qml/CatalogDetails.qml qml/StatCard.qml qml/RichDescription.qml qml/CollectionDetails.qml
        qml/HeroData.js qml/HeroGrid.qml qml/CatalogFilters.qml qml/CatalogLinks.qml qml/CatalogResource.qml qml/CatalogStatus.qml
        qml/LibraryPage.qml
        qml/LibrarySidebar.qml
        qml/LibraryView.qml
        qml/ModRow.qml qml/CatalogRow.qml qml/MediaCard.qml qml/ReadableSwitch.qml qml/StatusPill.qml qml/FilterChip.qml qml/SegmentedControl.qml qml/Disclosure.qml qml/SettingRow.qml qml/Callout.qml qml/DecoPlayPlaque.qml qml/SectionHeading.qml qml/HealthText.js qml/InstalledModsDialog.qml qml/GroupImportDialog.qml qml/TweaksPage.qml qml/CrosshairPanel.qml qml/AutoexecPanel.qml qml/BisectPanel.qml qml/BisectSetupDialog.qml qml/AutoexecCommands.js qml/ConfigReviewDialog.qml qml/SettingsPage.qml
        qml/MediaResultView.qml qml/MediaViewState.qml
        qml/ModPreview.qml qml/MediaViewer.qml qml/ViewerSession.qml qml/SoundPreview.qml
        qml/ModDetails.qml qml/ModRemovalDialog.qml
        qml/PerformancePanel.qml
    SOURCES
        src/textutils.h
        src/maintenancecontroller.h src/maintenancecontroller.cpp
        src/storagecontroller.h src/storagecontroller.cpp
        src/profilesharecontroller.h src/profilesharecontroller.cpp
        src/launchhealth.h src/launchhealth.cpp src/launchcontroller.h src/launchcontroller.cpp src/launchdata.h src/launchdata.cpp
        src/optioncontroller.h src/optioncontroller.cpp src/optiondata.h src/optiondata.cpp
        src/fontcontroller.h src/fontcontroller.cpp src/fontdata.h src/fontdata.cpp
        src/performancecontroller.h src/performancecontroller.cpp
        src/gameconfigcontroller.h src/gameconfigcontroller.cpp
        src/localimportcontroller.h src/localimportcontroller.cpp src/localartworkprovider.h src/localartworkprovider.cpp
        src/catalogdata.h src/catalogdata.cpp src/catalogcontroller.h src/catalogcontroller.cpp src/catalogmodel.h src/catalogmodel.cpp
        src/librarymediacontroller.h src/librarymediacontroller.cpp
        src/librarymodel.h src/librarymodel.cpp
        src/backendclient.h src/backendclient.cpp src/libraryrows.h src/libraryrows.cpp
        src/remotedata.h src/remotedata.cpp src/remotecontroller.h src/remotecontroller.cpp
        src/framemetrics.h src/framemetrics.cpp
        src/previewnetworkfactory.h src/previewnetworkfactory.cpp
        src/previewreply.h src/previewreply.cpp
        src/audiodecoder.h src/audiodecoder.cpp src/audiopreview.h src/audiopreview.cpp
)
target_link_libraries(modlock PRIVATE Qt6::Core Qt6::Gui Qt6::GuiPrivate Qt6::Qml Qt6::Quick Qt6::Network Qt6::Multimedia deadlock_styleplugin)
target_include_directories(modlock PRIVATE src)

include(CTest)
if(BUILD_TESTING)
    target_sources(modlock PRIVATE
        tests/gallerycapture.cpp tests/animationcheck.cpp tests/uicheck.cpp
        tests/crosshaircapture.cpp tests/downloadcapture.cpp tests/libraryflickercheck.cpp)
    target_compile_definitions(modlock PRIVATE MODLOCK_UI_TEST_DRIVERS)
    find_package(Python3 REQUIRED COMPONENTS Interpreter)

    function(modlock_add_test name)
        add_test(NAME "${name}" COMMAND "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/isolated-ui-check.py" ${ARGN})
    endfunction()

    add_executable(apply-warning-policy-check tests/apply-warning-policy.cpp)
    target_include_directories(apply-warning-policy-check PRIVATE src)
    target_link_libraries(apply-warning-policy-check PRIVATE Qt6::Core)
    modlock_add_test(apply-warning-policy-check "$<TARGET_FILE:apply-warning-policy-check>")
    set_tests_properties(apply-warning-policy-check PROPERTIES TIMEOUT 10)
    modlock_add_test(profile-share-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/profile-share-check.py" "${HELPER_BINARY}")
    set_tests_properties(profile-share-check PROPERTIES TIMEOUT 120)
    find_program(NODE_EXECUTABLE node)
    if(NODE_EXECUTABLE)
        modlock_add_test(ui-review-check "${NODE_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/ui-review-check.js")
    endif()
    modlock_add_test(crosshair-panel-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/crosshair-panel-check.py" "$<TARGET_FILE:modlock>" "${HELPER_BINARY}")
    set_tests_properties(crosshair-panel-check PROPERTIES TIMEOUT 60)
    modlock_add_test(item121-fix789-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/fix789-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(item121-fix789-ui-check PROPERTIES TIMEOUT 90)
    modlock_add_test(tweaks-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/tweaks-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(tweaks-ui-check PROPERTIES TIMEOUT 60)
    modlock_add_test(hero-picker-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/hero-picker-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(hero-picker-ui-check PROPERTIES TIMEOUT 60)
    modlock_add_test(browse-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/browse-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(browse-ui-check PROPERTIES TIMEOUT 60)
    modlock_add_test(viewer-session-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/viewer-session-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(viewer-session-check PROPERTIES TIMEOUT 100)
    modlock_add_test(viewer-data-change-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/viewer-session-check.py" "$<TARGET_FILE:modlock>" --data-change)
    set_tests_properties(viewer-data-change-check PROPERTIES TIMEOUT 30)
    add_executable(install-link-check tests/installlinkcheck.cpp src/installlink.cpp src/instancebroker.cpp src/instancebroker.h src/networktransport/remoteuri.cpp)
    target_include_directories(install-link-check PRIVATE src)
    target_link_libraries(install-link-check PRIVATE Qt6::Core Qt6::Network)
    modlock_add_test(install-link-check "$<TARGET_FILE:install-link-check>")
    modlock_add_test(instance-startup-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/instance-startup-check.py" "$<TARGET_FILE:modlock>")
    # Sequential fixture limits total 169 seconds; leave time for cleanup.
    set_tests_properties(instance-startup-check PROPERTIES TIMEOUT 210)
    modlock_add_test(install-link-ui-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/installlink-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(install-link-check PROPERTIES TIMEOUT 35)
    # The scenarios each have a bounded subprocess timeout; allow their total
    # runtime, including retained-download polling and the bubble fade check.
    set_tests_properties(install-link-ui-check PROPERTIES TIMEOUT 360)
    add_executable(remote-redirect-check tests/remote-redirect-check.cpp
        src/networktransport/remotetransfer.cpp src/networktransport/remoteworker.cpp
        src/networktransport/remoteuri.cpp)
    target_include_directories(remote-redirect-check PRIVATE src/networktransport)
    target_link_libraries(remote-redirect-check PRIVATE Qt6::Core Qt6::Network)
    modlock_add_test(remote-redirect-authority "$<TARGET_FILE:remote-redirect-check>")

    modlock_add_test(rich-description-ui-check "$<TARGET_FILE:modlock>" --demo --check-ui rich-description)
    set_tests_properties(rich-description-ui-check PROPERTIES TIMEOUT 30 ENVIRONMENT "QT_LOGGING_RULES=*.debug=false")
    modlock_add_test(hero-skins-ui-check "$<TARGET_FILE:modlock>" --demo --check-ui hero-skins)
    set_tests_properties(hero-skins-ui-check PROPERTIES TIMEOUT 20)
    add_library(palette-test-theme MODULE tests/palette-theme.cpp)
    target_link_libraries(palette-test-theme PRIVATE Qt6::GuiPrivate)
    set_target_properties(palette-test-theme PROPERTIES LIBRARY_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/palette-test-theme")
    modlock_add_test(appimage-palette-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/appimage-palette-check.py" "$<TARGET_FILE:modlock>" "$<TARGET_FILE_DIR:palette-test-theme>")
    set_tests_properties(appimage-palette-check PROPERTIES TIMEOUT 90)
    modlock_add_test(preferences-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/preferences-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(preferences-check PROPERTIES TIMEOUT 120)
    modlock_add_test(review-status-ui-check "$<TARGET_FILE:modlock>" --demo --check-ui review-status)
    modlock_add_test(helper-stop-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/helper-stop-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(helper-stop-ui-check PROPERTIES TIMEOUT 45)
    modlock_add_test(first-run-ui-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/first-run-ui-check.py" "$<TARGET_FILE:modlock>")
    modlock_add_test(first-play-ui-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/first-play-check.py" "$<TARGET_FILE:modlock>" "${HELPER_BINARY}")
    modlock_add_test(home-profile-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/home-profile-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(home-profile-ui-check PROPERTIES TIMEOUT 150)
    modlock_add_test(home-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/home-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(home-ui-check PROPERTIES TIMEOUT 90)
    modlock_add_test(home-catalog-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/home-catalog-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(home-catalog-ui-check PROPERTIES TIMEOUT 90)
    modlock_add_test(shared-controls-ui-check "$<TARGET_FILE:modlock>" --demo --check-ui shared-controls)
    set_tests_properties(shared-controls-ui-check PROPERTIES TIMEOUT 30)
    modlock_add_test(deco-chrome-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/deco-chrome-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(deco-chrome-ui-check PROPERTIES TIMEOUT 135)
    modlock_add_test(profile-menu-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/profile-menu-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(profile-menu-ui-check PROPERTIES TIMEOUT 120)
    modlock_add_test(poster-controls-ui-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/poster-controls-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(poster-controls-ui-check PROPERTIES TIMEOUT 90)
    modlock_add_test(spotlight-ui-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/spotlight-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(spotlight-ui-check PROPERTIES TIMEOUT 90)
    modlock_add_test(library-smoke "$<TARGET_FILE:modlock>" --demo --smoke-test)
    modlock_add_test(large-library-smoke "$<TARGET_FILE:modlock>" --demo --rows 10000 --smoke-test)
    modlock_add_test(animation-visible-motion "$<TARGET_FILE:modlock>" --demo --check-animation)
    modlock_add_test(native-library-integration "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/integrationcheck.py"
        --binary "$<TARGET_FILE:modlock>" --helper "${HELPER_BINARY}"
        --source-dir "${CMAKE_CURRENT_SOURCE_DIR}")
    modlock_add_test(native-mod-removal-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-mod-removal-check.py" "$<TARGET_FILE:modlock>" "${HELPER_BINARY}")
    set_tests_properties(native-mod-removal-check PROPERTIES TIMEOUT 120)
    modlock_add_test(native-play-poll-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-play-poll-check.py" "$<TARGET_FILE:modlock>" "${HELPER_BINARY}")
    set_tests_properties(native-play-poll-check PROPERTIES TIMEOUT 60)
    modlock_add_test(native-play-stale-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-play-stale-check.py" "${HELPER_BINARY}")
    set_tests_properties(native-play-stale-check PROPERTIES TIMEOUT 60)
    modlock_add_test(native-play-cancel-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-play-cancel-check.py" "$<TARGET_FILE:modlock>" "${HELPER_BINARY}")
    set_tests_properties(native-play-cancel-check PROPERTIES TIMEOUT 60)
    modlock_add_test(native-play-failure-cancel-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-play-failure-cancel-check.py" "${HELPER_BINARY}")
    set_tests_properties(native-play-failure-cancel-check PROPERTIES TIMEOUT 60)
    modlock_add_test(native-launch-retired-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-launch-retired-check.py" "${HELPER_BINARY}")
    set_tests_properties(native-launch-retired-check PROPERTIES TIMEOUT 30)
    modlock_add_test(native-launch-recovery-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-launch-recovery-check.py" "$<TARGET_FILE:modlock>" "${HELPER_BINARY}")
    set_tests_properties(native-launch-recovery-check PROPERTIES TIMEOUT 150)
    modlock_add_test(native-play-restart-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/native-play-restart-check.py" "$<TARGET_FILE:modlock>" "${HELPER_BINARY}")
    set_tests_properties(native-play-restart-check PROPERTIES TIMEOUT 240)
    modlock_add_test(native-write-integration "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/nativeintegrationcheck.py"
        --binary "$<TARGET_FILE:modlock>" --helper "${HELPER_BINARY}")
    modlock_add_test(storage-format-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/storage-cross-version-check.py" --helper "${HELPER_BINARY}")
    set_tests_properties(storage-format-check PROPERTIES TIMEOUT 90)
    modlock_add_test(round7-current-regressions "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/round7-review-regressions.py" "${HELPER_BINARY}")
    set_tests_properties(round7-current-regressions PROPERTIES TIMEOUT 60)
    modlock_add_test(round8-review-regressions "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/round8-review-regressions.py" "${HELPER_BINARY}")
    set_tests_properties(round8-review-regressions PROPERTIES TIMEOUT 120)
    modlock_add_test(round9-current-regressions "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/round9-review-regressions.py" "${HELPER_BINARY}")
    set_tests_properties(round9-current-regressions PROPERTIES TIMEOUT 60)
    modlock_add_test(round12-review-regressions "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/round12-review-regressions.py" --helper "${HELPER_BINARY}" --binary "$<TARGET_FILE:modlock>")
    set_tests_properties(round12-review-regressions PROPERTIES TIMEOUT 180)
    set_tests_properties(native-write-integration PROPERTIES TIMEOUT 300)
    modlock_add_test(auto-apply-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/auto-apply-ui-check.py" --binary "$<TARGET_FILE:modlock>" --helper "${HELPER_BINARY}")
    set_tests_properties(auto-apply-ui-check PROPERTIES TIMEOUT 180)
    modlock_add_test(local-import-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/controlled-helper.py" --run-local-ui "$<TARGET_FILE:modlock>")
    set_tests_properties(local-import-ui-check PROPERTIES TIMEOUT 90)
    modlock_add_test(library-flicker-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/library-flicker-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(library-flicker-check PROPERTIES TIMEOUT 120)
    modlock_add_test(media-demand-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/media-demand-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(media-demand-check PROPERTIES TIMEOUT 60)
    foreach(guard media-hidden-reuse media-reveal-reuse)
        modlock_add_test(${guard}-check "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/media-card-guard-check.py" "$<TARGET_FILE:modlock>" ${guard})
        set_tests_properties(${guard}-check PROPERTIES TIMEOUT 60)
    endforeach()
    modlock_add_test(helper-rust-tests "${CARGO_EXECUTABLE}" test --offline --locked
        --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/backend/Cargo.toml" --target-dir "${HELPER_TARGET_DIR}")
    set_tests_properties(library-smoke large-library-smoke animation-visible-motion native-library-integration PROPERTIES
        ENVIRONMENT "QT_LOGGING_RULES=*.debug=false"
        TIMEOUT 15
    )
    # Generous limits: these suites run 30-60 s alone and much longer on a loaded machine.
    set_tests_properties(native-library-integration PROPERTIES TIMEOUT 300)
    # The complete Rust suite runs serially (RUST_TEST_THREADS=1) in qualification
    # and has taken more than 1200 s; 1800 s leaves headroom without hiding hangs.
    set_tests_properties(helper-rust-tests PROPERTIES TIMEOUT 1800 WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/backend")
endif()

if(BUILD_TESTING)
    add_executable(model-check tests/modelcheck.cpp src/librarymodel.h src/librarymodel.cpp
        src/maintenancecontroller.h src/maintenancecontroller.cpp
        src/storagecontroller.h src/storagecontroller.cpp
        src/profilesharecontroller.h src/profilesharecontroller.cpp
        src/catalogdata.h src/catalogdata.cpp src/catalogcontroller.h src/catalogcontroller.cpp src/catalogmodel.h src/catalogmodel.cpp
        src/librarymediacontroller.h src/librarymediacontroller.cpp
        src/launchhealth.h src/launchhealth.cpp src/launchcontroller.h src/launchcontroller.cpp src/launchdata.h src/launchdata.cpp
        src/optioncontroller.h src/optioncontroller.cpp src/optiondata.h src/optiondata.cpp
        src/fontcontroller.h src/fontcontroller.cpp src/fontdata.h src/fontdata.cpp
        src/performancecontroller.h src/performancecontroller.cpp
        src/gameconfigcontroller.h src/gameconfigcontroller.cpp
        src/backendclient.h src/backendclient.cpp src/libraryrows.h src/libraryrows.cpp
        src/remotedata.h src/remotedata.cpp src/remotecontroller.h src/remotecontroller.cpp
        src/localimportcontroller.h src/localimportcontroller.cpp src/localartworkprovider.h src/localartworkprovider.cpp)
    target_include_directories(model-check PRIVATE src)
    target_link_libraries(model-check PRIVATE Qt6::Core Qt6::Qml Qt6::Quick Qt6::Network)
    target_compile_definitions(model-check PRIVATE TEST_SOURCE_DIR="${CMAKE_CURRENT_SOURCE_DIR}" TEST_HELPER_PATH="${HELPER_BINARY}")
    add_dependencies(model-check modlock-helper-build)
    modlock_add_test(downloads-ui-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/downloads-ui-check.py" "$<TARGET_FILE:modlock>")
    modlock_add_test(model-client-check "$<TARGET_FILE:model-check>")
    set_tests_properties(model-client-check PROPERTIES TIMEOUT 420)
    modlock_add_test(launch-settings-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/launch-settings-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(launch-settings-ui-check PROPERTIES TIMEOUT 180)
endif()

if(BUILD_TESTING)
    qt_add_executable(preview-lifetime-check tests/previewlifetimecheck.cpp
        src/previewreply.cpp src/previewreply.h src/previewnetworkfactory.cpp src/previewnetworkfactory.h)
    target_include_directories(preview-lifetime-check PRIVATE src)
    target_link_libraries(preview-lifetime-check PRIVATE Qt6::Core Qt6::Gui Qt6::Qml Qt6::Quick Qt6::Network)
    modlock_add_test(preview-lifetime-check "$<TARGET_FILE:preview-lifetime-check>")
    set_tests_properties(preview-lifetime-check PROPERTIES TIMEOUT 30)
    # Sound previews: production fetch/policy/decoder/controller with fake
    # network replies and fake output; never creates QAudioSink or QMediaDevices.
    qt_add_executable(audio-preview-check tests/audiopreviewcheck.cpp
        src/audiodecoder.h src/audiodecoder.cpp src/audiopreview.h src/audiopreview.cpp
        src/networktransport/remoteuri.h src/networktransport/remoteuri.cpp)
    target_include_directories(audio-preview-check PRIVATE src)
    target_link_libraries(audio-preview-check PRIVATE Qt6::Core Qt6::Network Qt6::Multimedia)
    target_compile_definitions(audio-preview-check PRIVATE AUDIO_FIXTURES="${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures/audio")
    modlock_add_test(audio-preview-check "$<TARGET_FILE:audio-preview-check>")
    set_tests_properties(audio-preview-check PROPERTIES TIMEOUT 60)
    modlock_add_test(browse-audio-ui-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/browse-audio-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(browse-audio-ui-check PROPERTIES TIMEOUT 120)
    modlock_add_test(preview-cache-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/preview-cache-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(preview-cache-check PROPERTIES TIMEOUT 60)
    modlock_add_test(state-safety-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/state-safety-check.py" "$<TARGET_FILE:model-check>")
    set_tests_properties(state-safety-check PROPERTIES TIMEOUT 180)
    modlock_add_test(control-gallery-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/control-gallery-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(control-gallery-check PROPERTIES TIMEOUT 40)
    modlock_add_test(motion-08-ui-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/motion-08-ui-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(motion-08-ui-check PROPERTIES TIMEOUT 150)
endif()

if(BUILD_TESTING)
    add_executable(font-identity-check tests/fontidentitycheck.cpp src/fontdata.cpp)
    target_include_directories(font-identity-check PRIVATE src)
    target_link_libraries(font-identity-check PRIVATE Qt6::Core)
    modlock_add_test(font-identity-check "$<TARGET_FILE:font-identity-check>")
endif()

set_source_files_properties(packaging/modlock.svg PROPERTIES QT_RESOURCE_ALIAS modlock.svg)
qt_add_resources(modlock branding PREFIX "/" FILES packaging/modlock.svg)

if(BUILD_TESTING)
modlock_add_test(modlock-paths-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/modlock-paths-check.py" "${HELPER_BINARY}" "$<TARGET_FILE:modlock>")
modlock_add_test(qml-bundle-check "${Python3_EXECUTABLE}"
    "${CMAKE_CURRENT_SOURCE_DIR}/tests/qml-bundle-check.py")
modlock_add_test(modlock-packaging-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/modlock-packaging-check.py")
endif()

if(BUILD_TESTING)
modlock_add_test(deployment-layout-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/deployment-layout-check.py" --helper "${HELPER_BINARY}")
set_tests_properties(deployment-layout-check PROPERTIES TIMEOUT 60)
endif()

if(BUILD_TESTING)
add_executable(apppaths-review-probe tests/apppaths-review-check.cpp)
target_include_directories(apppaths-review-probe PRIVATE src)
target_link_libraries(apppaths-review-probe PRIVATE Qt6::Core)
add_dependencies(apppaths-review-probe modlock-helper-build)
modlock_add_test(apppaths-review-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/apppaths-review-check.py" "$<TARGET_FILE:apppaths-review-probe>")
endif()

if(BUILD_TESTING)
modlock_add_test(qualification-helper-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/qualification-helper-check.py")
endif()

if(BUILD_TESTING)
    modlock_add_test(rename-policy-check "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/rename-policy-check.py"
        --helper "${HELPER_BINARY}" --binary "$<TARGET_FILE:modlock>")
    set_tests_properties(rename-policy-check PROPERTIES TIMEOUT 120)

    set(MODLOCK_OLD_HELPER "" CACHE FILEPATH "Pre-rename helper for storage, game and rename compatibility qualification")
    set(MODLOCK_REGRESSION_OLD_HELPER "" CACHE FILEPATH "Historical helper for round 8 and 12 writer checks")
    set(MODLOCK_V1_HELPER "" CACHE FILEPATH "Manifest v1 helper for the round 8 retirement check")
    foreach(input MODLOCK_OLD_HELPER MODLOCK_REGRESSION_OLD_HELPER MODLOCK_V1_HELPER)
        if(NOT "${${input}}" STREQUAL "" AND (NOT EXISTS "${${input}}" OR IS_DIRECTORY "${${input}}"))
            message(FATAL_ERROR "${input} must name a historical helper executable: ${${input}}")
        endif()
    endforeach()
    if(MODLOCK_OLD_HELPER)
        modlock_add_test(storage-cross-version-check "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/storage-cross-version-check.py"
            --helper "${HELPER_BINARY}" --binary "$<TARGET_FILE:modlock>" --old-helper "${MODLOCK_OLD_HELPER}")
        modlock_add_test(game-format-cross-version-check "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/game-format-cross-version-check.py"
            --helper "${HELPER_BINARY}" --binary "$<TARGET_FILE:modlock>" --old-helper "${MODLOCK_OLD_HELPER}")
        modlock_add_test(legacy-restore-check "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/legacy-restore-check.py"
            --helper "${HELPER_BINARY}" --binary "$<TARGET_FILE:modlock>" --old-helper "${MODLOCK_OLD_HELPER}")
        modlock_add_test(rename-policy-historical-check "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/rename-policy-check.py" --case never-applied-old-workspace
            --helper "${HELPER_BINARY}" --binary "$<TARGET_FILE:modlock>" --old-helper "${MODLOCK_OLD_HELPER}")
        set_tests_properties(storage-cross-version-check game-format-cross-version-check legacy-restore-check
            rename-policy-historical-check PROPERTIES TIMEOUT 120 LABELS historical)
    endif()
    if(MODLOCK_REGRESSION_OLD_HELPER)
        modlock_add_test(round8-historical-regressions "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/round8-review-regressions.py" "${HELPER_BINARY}" --old-helper "${MODLOCK_REGRESSION_OLD_HELPER}")
        modlock_add_test(round12-historical-regressions "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/round12-review-regressions.py" --case warnings
            --helper "${HELPER_BINARY}" --binary "$<TARGET_FILE:modlock>" --old-helper "${MODLOCK_REGRESSION_OLD_HELPER}")
        set_tests_properties(round8-historical-regressions round12-historical-regressions PROPERTIES TIMEOUT 120 LABELS historical)
    endif()
    if(MODLOCK_V1_HELPER)
        modlock_add_test(round8-manifest-v1-regression "${Python3_EXECUTABLE}"
            "${CMAKE_CURRENT_SOURCE_DIR}/tests/round8-review-regressions.py" "${HELPER_BINARY}" --v1-helper "${MODLOCK_V1_HELPER}")
        set_tests_properties(round8-manifest-v1-regression PROPERTIES TIMEOUT 120 LABELS historical)
    endif()
endif()

if(BUILD_TESTING)
    modlock_add_test(bootstrap-root-check "${Python3_EXECUTABLE}" "${CMAKE_SOURCE_DIR}/tests/bootstrap-root-check.py" "${HELPER_BINARY}")
endif()

if(BUILD_TESTING)
    modlock_add_test(appearance-preferences-check "${Python3_EXECUTABLE}"
        "${CMAKE_CURRENT_SOURCE_DIR}/tests/appearance-preferences-check.py" "$<TARGET_FILE:modlock>")
    set_tests_properties(appearance-preferences-check PROPERTIES TIMEOUT 60)
endif()
