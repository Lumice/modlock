#pragma once
#include <QAccessible>
#include <QCoreApplication>
#include <QDir>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QGuiApplication>
#include <QMouseEvent>
#include <QPalette>
#include <QQuickItem>
#include <QQuickWindow>
#include <QTimer>

namespace {

inline void applyProfileMenuPalette(bool dark) {
    auto palette = QGuiApplication::palette();
    palette.setColor(QPalette::Window, QColor(dark ? "#0e0c15" : "#f1eff5"));
    palette.setColor(QPalette::Base, QColor(dark ? "#120f1b" : "#ffffff"));
    palette.setColor(QPalette::AlternateBase, QColor(dark ? "#14111d" : "#e9e6ef"));
    palette.setColor(QPalette::Button, QColor(dark ? "#1a1626" : "#fbfafd"));
    const QColor text(dark ? "#eee8dc" : "#1b1626");
    const QColor disabled(dark ? "#716a7b" : "#9b94a6");
    for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) {
        palette.setColor(role, text);
        palette.setColor(QPalette::Disabled, role, disabled);
    }
    QGuiApplication::setPalette(palette);
}

inline void saveProfileMenuCapture(QQuickWindow *window, const QString &name, int width = 0, int height = 0) {
    const auto dir = qEnvironmentVariable("PROFILE_MENU_CAPTURES");
    if (dir.isEmpty()) return;
    QDir().mkpath(dir);
    if (width > 0 && height > 0) {
        window->resize(width, height);
        decoSettle(6, 30);
    } else {
        decoSettle(3, 25);
    }
    require(window->grabWindow().save(dir + "/" + name + ".png"), qPrintable("Profile menu capture failed: " + name));
}

inline void saveOpenChooserCaptures(QQuickWindow *window, QObject *popup, const QString &prefix) {
    if (qEnvironmentVariableIsEmpty("PROFILE_MENU_CAPTURES")) return;
    for (const int width : {1200, 720}) {
        window->resize(width, 780);
        decoSettle(3, 25);
        if (!popup->property("visible").toBool()) {
            realClick(window, "profileChooser");
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "profile popup failed to reopen for capture");
        }
        auto *popupItem = qvariant_cast<QQuickItem *>(popup->property("popupItem"));
        if (popupItem) {
            decoWaitFor([&] { return popupItem->opacity() >= 0.99; }, 1000);
        }
        saveProfileMenuCapture(window, prefix + "-" + QString::number(width));
    }
    window->resize(1200, 780);
    decoSettle(3, 25);
}

inline QQuickItem *findVisibleProfileOption(QQuickWindow *window, const QString &prefix) {
    for (auto *obj : objects(window)) {
        if (obj->objectName() == "profileOption") {
            auto *item = qobject_cast<QQuickItem *>(obj);
            if (item && item->isVisible() && item->property("text").toString().startsWith(prefix))
                return item;
        }
    }
    return nullptr;
}

inline void verifyOutsideProfileClick(QQuickWindow *window, QObject *popup) {
    realKey(window, Qt::Key_Escape);
    realClick(window, "libraryTab");
    require(decoWaitFor([&] { auto *o=findNamed(window,"modSearch"); return o && o->property("visible").toBool(); }), "Library search not ready");
    realClick(window, "profileChooser");
    require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Chooser did not open for outside click");
    realClick(window, "modSearch");
    decoSettle(6, 30);
    auto *search=named(window,"modSearch");
    require(!popup->property("visible").toBool() && search->property("activeFocus").toBool(), "Chooser close stole outside search focus");
    realKey(window, Qt::Key_X, Qt::NoModifier, "x");
    require(search->property("text").toString()=="x", "Outside search did not receive typing");
    realKey(window, Qt::Key_A, Qt::ControlModifier);
    realKey(window, Qt::Key_Backspace);
    realClick(window, "homeTab");
    realClick(window, "profileChooser");
    require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Chooser did not reopen after outside click check");
}

inline void startProfileMenuCheck(QQuickWindow *window, LibraryModel *model) {
    const QString mode = qEnvironmentVariable("PROFILE_MENU_MODE", "native");
    const QString themeEnv = qEnvironmentVariable("PROFILE_MENU_THEME", "dark");
    applyProfileMenuPalette(themeEnv != "light");

    window->resize(1200, 780);
    auto *timer = new QTimer(window);
    timer->setInterval(60);
    QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, mode, themeEnv, ticks = 0]() mutable {
        try {
            require(++ticks < 200, "profile-menu check timed out waiting for model");
            if (model->loading() || !window->isVisible() || model->profiles().isEmpty()) return;
            if (mode == "native" && (!model->nativeMode() || !model->canEditProfiles())) return;
            if (mode == "readonly" && (model->nativeMode() || !model->readOnly() || !model->canBrowseProfiles() || model->canEditProfiles())) return;

            timer->stop();

            auto *chooser = qobject_cast<QQuickItem *>(named(window, "profileChooser"));
            require(chooser->isVisible() && chooser->isEnabled(), "profileChooser must be visible and enabled");
            require(chooser->width() >= 24 && chooser->height() >= 24, "profileChooser hit target below 24px");

            auto *popup = chooser->property("popup").value<QObject *>();
            require(popup && !popup->property("visible").toBool(), "Profile picker missing or already open");

            // Open the chooser and verify its profile options and management action.
            realClick(window, "profileChooser");
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "profileChooser pointer click did not open popup");

            auto *list = qobject_cast<QQuickItem *>(findNamed(window, "profilePopupList"));
            auto *separator = qobject_cast<QQuickItem *>(findNamed(window, "profilePopupSeparator"));
            auto *manageItem = qobject_cast<QQuickItem *>(findNamed(window, "manageProfilesMenuItem"));
            require(list && list->isVisible(), "profilePopupList missing or hidden in open popup");
            require(separator && separator->isVisible(), "profilePopupSeparator missing or hidden in open popup");
            require(manageItem && manageItem->isVisible(), "manageProfilesMenuItem missing or hidden in open popup");

            const int expectedCount = model->profiles().size();
            require(list->property("count").toInt() == expectedCount, "profilePopupList includes unexpected fake row");
            require(manageItem->property("text").toString() == "Manage profiles", "manageProfilesMenuItem text mismatch");
            auto *accessibleManage = QAccessible::queryAccessibleInterface(manageItem);
            require(accessibleManage && accessibleManage->text(QAccessible::Name) == "Manage profiles", "manageProfilesMenuItem accessible name mismatch");
            require(manageItem->width() >= 24 && manageItem->height() >= 24, "manageProfilesMenuItem hit target below 24px");

            const auto listRect = list->mapRectToScene(list->boundingRect());
            const auto sepRect = separator->mapRectToScene(separator->boundingRect());
            const auto manageRect = manageItem->mapRectToScene(manageItem->boundingRect());
            require(listRect.bottom() <= sepRect.top() + 1.0, "profilePopupList overlaps separator");
            require(sepRect.bottom() <= manageRect.top() + 1.0, "profilePopupSeparator overlaps manageProfilesMenuItem");
            require(manageItem->width() >= manageItem->property("implicitWidth").toDouble() - 1.0,
                    "manageProfilesMenuItem width smaller than implicitWidth (risk of text truncation)");

            // Capture open menu across 1200px and 720px widths (long profile name + separator + action).
            saveOpenChooserCaptures(window, popup, mode + "-menu-open-" + themeEnv);
            if (!popup->property("visible").toBool()) {
                realClick(window, "profileChooser");
                require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "popup did not reopen after captures");
            }

            verifyOutsideProfileClick(window, popup);
            // Real keyboard selection beyond the visible nine-row viewport.
            chooser->forceActiveFocus(Qt::TabFocusReason);
            realKey(window, Qt::Key_End);
            decoSettle(4, 25);
            auto *last = findVisibleProfileOption(window, "Profile 15");
            require(last && list->mapRectToScene(list->boundingRect()).contains(last->mapToScene(last->boundingRect().center())), "End highlighted row is outside the list viewport");
            realKey(window, Qt::Key_Return);
            require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId()=="p15"; }), "Keyboard End/Return did not select profile15");
            require(model->activeProfileId()=="p1", "Keyboard selection changed active profile");
            realClick(window, "profileChooser");
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Large list did not reopen");
            decoSettle(4,25);
            auto *nearLast = findVisibleProfileOption(window, "Profile 14");
            require(nearLast && list->mapRectToScene(list->boundingRect()).contains(nearLast->mapToScene(nearLast->boundingRect().center())), "Scrolled pointer target is outside viewport");
            auto *accessibleRow=QAccessible::queryAccessibleInterface(nearLast);
            require(accessibleRow && accessibleRow->role()==QAccessible::ListItem && accessibleRow->text(QAccessible::Name)=="Profile 14", "Profile row accessible role/name mismatch");
            require(accessibleManage->role()==QAccessible::Button, "Manage accessible button role missing");
            if (manageItem->isEnabled())
                require(accessibleManage->actionInterface() && accessibleManage->actionInterface()->actionNames().contains(QAccessibleActionInterface::pressAction()), "Manage accessible button has no press action");
            realClickItem(window, nearLast);
            require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId()=="p14"; }), "Scrolled pointer selection failed");
            realClick(window, "profileChooser");
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Chooser did not reopen for mixed input");
            decoSettle(4,25);
            last=findVisibleProfileOption(window,"Profile 15");
            require(last, "Mixed-input hover target missing");
            const auto hoverPos=last->mapToScene(last->boundingRect().center());
            QMouseEvent move(QEvent::MouseMove,hoverPos,hoverPos,window->mapToGlobal(hoverPos.toPoint()),Qt::NoButton,Qt::NoButton,Qt::NoModifier);
            QCoreApplication::sendEvent(window,&move);
            realKey(window,Qt::Key_Home);
            realKey(window,Qt::Key_Down);
            realKey(window,Qt::Key_Return);
            require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId()=="p2"; }), "Keyboard navigation after pointer hover selected wrong profile");
            realClick(window,"profileChooser");
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Chooser did not reopen for reset");
            chooser->forceActiveFocus(Qt::TabFocusReason);
            realKey(window,Qt::Key_Home);realKey(window,Qt::Key_Return);
            require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId()=="p1"; }), "Keyboard Home/Return did not select first profile");
            realClick(window,"profileChooser");
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Chooser did not reopen after reset");
            if (mode == "readonly") {
                require(!manageItem->isEnabled(), "manageProfilesMenuItem enabled while canEditProfiles is false");
                // Send real pointer press & release to disabled menu item; dialog must NOT open.
                const auto p = manageItem->mapToScene(QPointF(manageItem->width() / 2, manageItem->height() / 2));
                QMouseEvent press(QEvent::MouseButtonPress, p, p, window->mapToGlobal(p.toPoint()), Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
                QMouseEvent release(QEvent::MouseButtonRelease, p, p, window->mapToGlobal(p.toPoint()), Qt::LeftButton, Qt::NoButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &press);
                QCoreApplication::sendEvent(window, &release);
                decoSettle(4, 25);
                require(!profilePopup(window) || !profilePopup(window)->property("visible").toBool(),
                        "Clicking disabled manageProfilesMenuItem opened ProfilesDialog");

                // Keyboard navigation must not enter disabled item or trap Tab.
                if (!popup->property("visible").toBool()) {
                    realClick(window, "profileChooser");
                    require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "popup did not reopen for keyboard check");
                }
                chooser->forceActiveFocus(Qt::TabFocusReason);
                realKey(window, Qt::Key_End);
                realKey(window, Qt::Key_Down);
                require(!manageItem->hasActiveFocus(), "Key_Down focused disabled manageProfilesMenuItem");
                realKey(window, Qt::Key_Tab);
                require(!popup->property("visible").toBool() && !manageItem->hasActiveFocus() && !chooser->hasActiveFocus(),
                        "Tab trapped inside popup while manageProfilesMenuItem disabled");

                // Profile selection must STILL work while profile editing is disabled.
                realClick(window, "profileChooser");
                QQuickItem *p2Opt = nullptr;
                require(decoWaitFor([&] {
                    if (!popup->property("visible").toBool()) return false;
                    p2Opt = findVisibleProfileOption(window, "Extremely Long");
                    return p2Opt != nullptr;
                }), "Long profile option p2 not found in open readonly popup");
                realClickItem(window, p2Opt);
                require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId() == "p2"; }),
                        "Real pointer profile selection failed in readonly mode");
                require(!popup->property("visible").toBool(),
                        "Popup stayed open after profile selection");
                require(model->activeProfileId() == "p1", "Profile selection altered activeProfileId");
                require(!model->canEditProfiles(), "Profile selection unexpectedly enabled editing");

                std::puts("PASS profile-menu readonly");
                QCoreApplication::exit(0);
                return;
            }

            // Native mode checks:
            require(manageItem->isEnabled(), "manageProfilesMenuItem disabled in writable native mode");

            // 3. Real pointer profile selection (viewProfile) + active-vs-viewed distinction.
            auto *p2Opt = findVisibleProfileOption(window, "Extremely Long");
            require(p2Opt != nullptr, "Long profile option p2 not found in open native popup");
            realClickItem(window, p2Opt);
            require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId() == "p2"; }),
                    "Real pointer profile selection failed in native mode");
            require(!popup->property("visible").toBool(),
                    "Chooser popup remained open after selecting profile");
            require(model->activeProfileId() == "p1", "Selecting profile altered activeProfileId");

            // 4. Real pointer activation of Manage profiles -> ProfilesDialog opens, popup close does not steal focus, Escape closes -> focus returns to profileChooser.
            realClick(window, "profileChooser");
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "popup did not reopen for pointer Manage profiles");
            realClickItem(window, manageItem);
            require(decoWaitFor([&] { return profilePopup(window) && profilePopup(window)->property("visible").toBool(); }),
                    "Pointer click on manageProfilesMenuItem did not open ProfilesDialog");
            require(!popup->property("visible").toBool(), "Chooser popup stayed open when ProfilesDialog opened");
            decoSettle(4, 25);
            auto *nameField = qobject_cast<QQuickItem *>(named(window, "profileNameField"));
            require(nameField && nameField->hasActiveFocus(),
                    "Closing chooser popup stole focus from ProfilesDialog profileNameField");
            saveProfileMenuCapture(window, "native-dialog-active-" + themeEnv + "-1200", 1200, 780);
            saveProfileMenuCapture(window, "native-dialog-active-" + themeEnv + "-720", 720, 780);
            window->resize(1200, 780);
            decoSettle(3, 25);

            // Close ProfilesDialog and prove focus returns to valid visible profileChooser.
            realKey(window, Qt::Key_Escape);
            require(decoWaitFor([&] { return !profilePopup(window) || !profilePopup(window)->property("visible").toBool(); }),
                    "Escape did not close ProfilesDialog");
            require(decoWaitFor([&] { return chooser->hasActiveFocus(); }),
                    "Focus did not return to profileChooser after ProfilesDialog closed");
            saveProfileMenuCapture(window, "native-focus-return-" + themeEnv + "-1200");
            require(chooser->hasActiveFocus() && chooser->property("visualFocus").toBool(), "Dialog Escape return lacks visible chooser focus");
            auto *focusBackground=chooser->property("background").value<QQuickItem *>();
            require(QQmlProperty(focusBackground,"border.width").read().toInt()==2, "Returned chooser focus border missing");

            // 5. Keyboard Escape dismisses open chooser menu and keeps focus on profileChooser.
            chooser->forceActiveFocus(Qt::TabFocusReason);
            realKey(window, Qt::Key_Space);
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Space did not open profileChooser popup");
            realKey(window, Qt::Key_Escape);
            require(decoWaitFor([&] { return !popup->property("visible").toBool(); }), "Escape did not dismiss profileChooser popup");
            decoSettle(2, 20);
            require(chooser->hasActiveFocus(), "Escape from open popup lost focus from profileChooser");

            // 6. Keyboard Down from last option -> Manage profiles -> Up back to options -> Down -> Return opens ProfilesDialog -> Escape returns focus to chooser.
            realKey(window, Qt::Key_Space);
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Space did not reopen popup for arrow nav");
            realKey(window, Qt::Key_End);
            require(chooser->property("highlightedIndex").toInt() == expectedCount - 1, "Key_End did not highlight last profile option");
            realKey(window, Qt::Key_Down);
            require(manageItem->hasActiveFocus(), "Key_Down from last profile option did not focus manageProfilesMenuItem");
            realKey(window, Qt::Key_Up);
            require(chooser->hasActiveFocus() && popup->property("visible").toBool(), "Key_Up from manageProfilesMenuItem did not return focus to profileChooser");
            realKey(window, Qt::Key_Down);
            require(manageItem->hasActiveFocus(), "Second Key_Down did not focus manageProfilesMenuItem");
            realKey(window, Qt::Key_Return);
            require(decoWaitFor([&] { return profilePopup(window) && profilePopup(window)->property("visible").toBool(); }),
                    "Key_Return on manageProfilesMenuItem did not open ProfilesDialog");
            decoSettle(4, 25);
            require(named(window, "profileNameField")->property("activeFocus").toBool(),
                    "ProfilesDialog opened via keyboard did not retain focus on profileNameField");
            realKey(window, Qt::Key_Escape);
            require(decoWaitFor([&] { return !profilePopup(window) || !profilePopup(window)->property("visible").toBool(); }),
                    "Escape did not close keyboard-opened ProfilesDialog");
            require(decoWaitFor([&] { return chooser->hasActiveFocus(); }),
                    "Focus did not return to profileChooser after keyboard dialog close");

            decoSettle(4,25);
            require(chooser->hasActiveFocus() && chooser->property("visualFocus").toBool(), "Keyboard-opened dialog return lacks visible focus");

            // 7. Keyboard Tab to manageProfilesMenuItem -> Backtab back -> Tab again dismisses popup without Tab trap.
            chooser->forceActiveFocus(Qt::TabFocusReason);
            realKey(window, Qt::Key_Space);
            require(decoWaitFor([&] { return popup->property("visible").toBool(); }), "Space did not reopen popup for Tab check");
            realKey(window, Qt::Key_Tab);
            require(manageItem->hasActiveFocus(), "Key_Tab inside open popup did not focus manageProfilesMenuItem");
            realKey(window, Qt::Key_Backtab);
            require(chooser->hasActiveFocus() && popup->property("visible").toBool(), "Key_Backtab did not return focus to profileChooser");
            realKey(window, Qt::Key_Tab);
            require(manageItem->hasActiveFocus(), "Second Key_Tab did not focus manageProfilesMenuItem");
            realKey(window, Qt::Key_Tab);
            require(decoWaitFor([&] { return !popup->property("visible").toBool(); }), "Key_Tab from manageProfilesMenuItem did not close popup");
            require(!manageItem->hasActiveFocus() && !chooser->hasActiveFocus(), "Key_Tab trapped focus in profile controls");

            auto *preferences = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
            const bool reduced = preferences->property("reduceMotion").toBool();
            preferences->setProperty("reduceMotion", true);
            realClick(window, "profileChooser");
            QCoreApplication::processEvents();
            require(popup->property("opened").toBool(), "Reduced-motion profile chooser is still animating after opening");
            realKey(window, Qt::Key_Escape);
            preferences->setProperty("reduceMotion", reduced);

            std::puts("PASS profile-menu native");
            QCoreApplication::exit(0);
        } catch (const std::exception &error) {
            std::fprintf(stderr, "FAIL profile-menu (%s): %s\n", qPrintable(mode), error.what());
            QCoreApplication::exit(1);
        }
    });
    timer->start();
}

} // namespace
