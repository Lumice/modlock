// SPDX-License-Identifier: GPL-3.0-or-later
// KDE role/state/shade formulas follow KColorScheme and KColorUtils (LGPL-2.0-or-later).
// Original KDE authors: Matthew Woehlke, Thomas Zander, Zack Rusin, Olaf Schmidt.
#include "appimagepalette.h"

#include <QDir>
#include <QGuiApplication>
#include <QHash>
#include <QPalette>
#include <QFile>
#include <fcntl.h>
#include <private/qguiapplication_p.h>
#include <qpa/qplatformtheme.h>
#include <sys/stat.h>
#include <unistd.h>
#include <cmath>

void applyAppImagePalette(Qt::ColorScheme systemScheme)
{
    if (!qEnvironmentVariableIsSet("APPIMAGE")) return;
    // Qt's generic fallback theme always supplies its stock palette, so "has a
    // palette" isn't enough: a real theme (KDE or otherwise) is one whose
    // palette differs from the base QPlatformTheme default.
    const auto *theme = QGuiApplicationPrivate::platformTheme();
    const QPalette *supplied = theme ? theme->palette(QPlatformTheme::SystemPalette) : nullptr;
    const QPlatformTheme stockTheme;
    const QPalette *stock = stockTheme.palette(QPlatformTheme::SystemPalette);
    if (supplied && !(stock && *supplied == *stock)) return;
    // Stand in for the KDE theme plugin only: the user asked for it (directly
    // or by running KDE), and the AppImage can't load it. Any other request
    // keeps Qt's own palette, including its dark Fusion palette.
    const bool kdeRequested = qEnvironmentVariable("QT_QPA_PLATFORMTHEME").contains("kde", Qt::CaseInsensitive)
        || qEnvironmentVariable("XDG_CURRENT_DESKTOP").contains("KDE", Qt::CaseInsensitive);
    if (!kdeRequested) return;

    constexpr qint64 cap = 1024 * 1024;
    auto config = qEnvironmentVariable("XDG_CONFIG_HOME");
    if (config.isEmpty()) config = QDir::homePath() + "/.config";
    const auto path = QFile::encodeName(QDir(config).filePath("kdeglobals"));
    // O_NONBLOCK closes the stat/open FIFO race; fstat checks the opened file.
    const int fd = ::open(path.constData(), O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    QByteArray bytes;
    if (fd >= 0) {
        struct stat st{};
        if (::fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_size <= cap) {
            QFile file;
            if (file.open(fd, QIODevice::ReadOnly, QFileDevice::DontCloseHandle)) {
                bytes = file.read(cap + 1);
                if (bytes.size() > cap || file.error() != QFileDevice::NoError) bytes.clear();
            }
        }
        ::close(fd);
    }
    QHash<QString, QHash<QString, QColor>> groups;
    QHash<QString, QHash<QString, QString>> settings;
    QString group;
    // Parse only literal RGB/RGBA colours; no QSettings writes, includes,
    // expansion, or unbounded reads. Ignore malformed and unrelated entries.
    for (const auto &raw : bytes.split('\n')) {
        const auto line = QString::fromUtf8(raw).trimmed();
        if (line.startsWith('[') && line.endsWith(']')) {
            group = line.mid(1, line.size() - 2);
            continue;
        }
        const auto eq = line.indexOf('=');
        if (eq < 1) continue;
        // KDE treats an empty value ("Color=") as unset, not as invalid, and a
        // later line overrides an earlier one: an empty value clears the key.
        if (line.mid(eq + 1).trimmed().isEmpty()) {
            settings[group].remove(line.left(eq).trimmed());
            groups[group].remove(line.left(eq).trimmed());
            continue;
        }
        settings[group].insert(line.left(eq).trimmed(), line.mid(eq + 1).trimmed());
        if (!group.startsWith("Colors:") && !group.startsWith("ColorEffects:")) continue;
        groups[group].remove(line.left(eq).trimmed());
        const auto parts = line.mid(eq + 1).split(',');
        if (parts.size() != 3 && parts.size() != 4) continue;
        int rgba[4] = {0, 0, 0, 255};
        bool valid = true;
        for (int i = 0; i < parts.size(); ++i) {
            bool ok = false;
            rgba[i] = parts[i].trimmed().toInt(&ok);
            valid &= ok && rgba[i] >= 0 && rgba[i] <= 255;
        }
        if (valid) groups[group].insert(line.left(eq).trimmed(), QColor(rgba[0], rgba[1], rgba[2], rgba[3]));
    }
    // Role assignments follow KColorScheme::createApplicationPalette:
    // https://invent.kde.org/frameworks/kcolorscheme/-/blob/master/src/kcolorscheme.cpp
    // Complementary and DecorationFocus/Hover have no QPalette equivalents;
    // don't substitute them for selection (Tokens.focus derives from accent).
    struct Role { QPalette::ColorRole role; const char *set; const char *key; };
    const Role roles[] = {
        {QPalette::Window, "Window", "BackgroundNormal"}, {QPalette::WindowText, "Window", "ForegroundNormal"},
        {QPalette::Base, "View", "BackgroundNormal"}, {QPalette::Text, "View", "ForegroundNormal"},
        {QPalette::AlternateBase, "View", "BackgroundAlternate"}, {QPalette::PlaceholderText, "View", "ForegroundInactive"},
        {QPalette::Link, "View", "ForegroundLink"}, {QPalette::LinkVisited, "View", "ForegroundVisited"},
        {QPalette::Button, "Button", "BackgroundNormal"}, {QPalette::ButtonText, "Button", "ForegroundNormal"},
        {QPalette::Highlight, "Selection", "BackgroundNormal"}, {QPalette::Accent, "Selection", "BackgroundNormal"},
        {QPalette::HighlightedText, "Selection", "ForegroundNormal"},
        {QPalette::ToolTipBase, "Tooltip", "BackgroundNormal"}, {QPalette::ToolTipText, "Tooltip", "ForegroundNormal"}
    };
    bool scheme = true;
    for (const auto &role : roles) {
        const QString name = "Colors:" + QString::fromLatin1(role.set);
        scheme &= groups.value(name).contains(role.key);
        if (settings.contains(name + "][Inactive") && QString::fromLatin1(role.set) != "Tooltip")
            scheme &= groups.value(name + "][Inactive").contains(role.key);
    }
    for (const auto &name : {"ColorEffects:Inactive", "ColorEffects:Disabled"})
        if (settings.value(name).contains("Color")) scheme &= groups.value(name).contains("Color");
    if (!scheme && systemScheme != Qt::ColorScheme::Dark) return;

    // Incomplete files never contribute colours or effects to the fallback.
    const bool dark = !scheme || groups["Colors:Window"]["BackgroundNormal"].lightnessF() < 0.5;
    QPalette palette = dark ? QPalette(QColor("#31363b")) : QGuiApplication::palette();
    if (!scheme) {
        palette.setColor(QPalette::WindowText, QColor("#eff0f1"));
        palette.setColor(QPalette::Text, QColor("#eff0f1"));
        palette.setColor(QPalette::ButtonText, QColor("#eff0f1"));
        palette.setColor(QPalette::Base, QColor("#232629"));
        palette.setColor(QPalette::AlternateBase, QColor("#2a2e32"));
        palette.setColor(QPalette::Highlight, QColor("#1d6fa5"));
        palette.setColor(QPalette::Accent, QColor("#1d6fa5"));
        palette.setColor(QPalette::HighlightedText, QColor("#eff0f1"));
        palette.setColor(QPalette::ToolTipBase, QColor("#31363b"));
        palette.setColor(QPalette::ToolTipText, QColor("#eff0f1"));
        palette.setColor(QPalette::PlaceholderText, QColor("#a1a9b1"));
        palette.setColor(QPalette::Link, QColor("#2980b9"));
        palette.setColor(QPalette::LinkVisited, QColor("#9b59b6"));
        for (auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText,
                          QPalette::HighlightedText, QPalette::ToolTipText, QPalette::PlaceholderText})
            palette.setColor(QPalette::Disabled, role, QColor("#c0c7ce"));
        QGuiApplication::setPalette(palette);
        return;
    }
    auto number = [&](const QString &section, const QString &key, double fallback) {
        bool ok = false;
        const auto value = settings.value(section).value(key).toDouble(&ok);
        return ok && std::isfinite(value) ? value : fallback;
    };
    auto enabled = [&](const QString &section, const QString &key, bool fallback) {
        const auto value = settings.value(section).value(key).toLower();
        return value == "true" || value == "1" ? true : value == "false" || value == "0" ? false : fallback;
    };
    auto mix = [](QColor a, QColor b, double t) {
        t = qBound(0., t, 1.);
        return QColor::fromRgbF(a.redF() * (1-t) + b.redF() * t, a.greenF() * (1-t) + b.greenF() * t,
                               a.blueF() * (1-t) + b.blueF() * t, a.alphaF() * (1-t) + b.alphaF() * t);
    };
    // HCY: hue/chroma with gamma-linear Rec.709 luma, as used by KDE.
    struct Hcy { double h, c, y, a; };
    auto hcy = [](QColor color) {
        const double r=std::pow(color.redF(),2.2), g=std::pow(color.greenF(),2.2), b=std::pow(color.blueF(),2.2);
        const double y=.2126*r+.7152*g+.0722*b;
        const double hi=qMax(r,qMax(g,b)), lo=qMin(r,qMin(g,b));
        double h=0;
        if (hi != lo) {
            const double d=6*(hi-lo);
            h = hi == r ? (g-b)/d : hi == g ? (b-r)/d+1./3 : (r-g)/d+2./3;
            if (h < 0) h += 1;
        }
        return Hcy{h, hi == lo ? 0. : qMax((y-lo)/y,(hi-y)/(1-y)),y,color.alphaF()};
    };
    auto rgb = [](Hcy v) {
        v.c=qBound(0.,v.c,1.); v.y=qBound(0.,v.y,1.);
        const double h=v.h*6, x=1-std::abs(std::fmod(h,2.)-1);
        const double r=h<1?1:h<2?x:h<4?0:h<5?x:1;
        const double g=h<1?x:h<3?1:h<4?x:0;
        const double b=h<2?0:h<3?x:h<5?1:x;
        const double z=.2126*r+.7152*g+.0722*b;
        const double scale=v.c*qMin(v.y/z,(1-v.y)/(1-z));
        // Round half up at the final 8-bit boundary, without quantizing the
        // intermediate linear channels/hue through QColor.
        auto channel=[&](double c) { return qRound(255*std::pow(qBound(0.,v.y+scale*(c-z),1.),1/2.2)); };
        return QColor(channel(r),channel(g),channel(b),qRound(v.a*255));
    };
    auto tint = [&](QColor base, QColor color, double amount) {
        if (amount <= 0) return base;
        if (amount >= 1) return color;
        const double y = hcy(base).y;
        auto ratio = [](double a, double b) { return (qMax(a,b)+.05)/(qMin(a,b)+.05); };
        const double target = 1 + (ratio(y, hcy(color).y)+1) * amount * amount * amount;
        double lo=0, hi=1, t=.5;
        Hcy result{};
        for (int i=0; i<12; ++i) {
            t=(lo+hi)/2;
            result=hcy(mix(base,color,std::pow(t,.3)));
            result.y=y*(1-t)+result.y*t;
            if (ratio(y,result.y)>target) hi=t; else lo=t;
        }
        return rgb(result);
    };
    for (const auto state : {QPalette::Active, QPalette::Inactive, QPalette::Disabled}) {
        for (const auto &role : roles) {
            QString set = role.set;
            const bool selection = set == "Selection";
            const bool inactiveSelection = state == QPalette::Inactive && selection
                && enabled("ColorEffects:Inactive", "ChangeSelectionColor", enabled("ColorEffects:Inactive", "Enable", true));
            const bool tooltip = set == "Tooltip";
            // KDE disabled selections use Window colours, never the accent.
            if ((state == QPalette::Disabled && selection) || inactiveSelection) set = "Window";
            const auto name = "Colors:" + set;
            const auto active = groups.value(name);
            const auto values = state == QPalette::Inactive && !tooltip && groups.contains(name + "][Inactive")
                ? groups.value(name + "][Inactive") : active;
            auto fallbackRole = role.role;
            if (selection && set == "Window") fallbackRole = QByteArray(role.key).startsWith("Background") ? QPalette::Window : QPalette::WindowText;
            QColor color = values.value(role.key, palette.color(QPalette::Active, fallbackRole));
            const auto backgroundRole = set == "View" ? QPalette::Base : set == "Button" ? QPalette::Button : QPalette::Window;
            auto bg = values.value("BackgroundNormal", palette.color(QPalette::Active, backgroundRole));
            if (inactiveSelection) {
                const auto accent = groups.value("Colors:Selection").value("BackgroundNormal", palette.color(QPalette::Active, QPalette::Highlight));
                if (QByteArray(role.key).startsWith("Background")) color = tint(color, accent, .4);
                bg = tint(bg, accent, .4);
            }
            const auto effectGroup = state == QPalette::Disabled ? "ColorEffects:Disabled" : "ColorEffects:Inactive";
            if (state != QPalette::Active && !tooltip && enabled(effectGroup, "Enable", state == QPalette::Disabled)) {
                const bool disabled = state == QPalette::Disabled;
                const auto effectColor = groups.value(effectGroup).value("Color", disabled ? QColor(56,56,56) : QColor(112,111,110));
                const int contrast = number(effectGroup, "ContrastEffect", disabled ? 1 : 2);
                const double contrastAmount = number(effectGroup, "ContrastAmount", disabled ? .65 : .25);
                if (QByteArray(role.key).startsWith("Foreground")) {
                    if (contrast == 1) color = mix(color, bg, contrastAmount);
                    if (contrast == 2) color = tint(color, bg, contrastAmount);
                }
                auto v = hcy(color);
                const int intensity = number(effectGroup, "IntensityEffect", disabled ? 2 : 0);
                const double amount = number(effectGroup, "IntensityAmount", disabled ? .1 : 0);
                if (intensity == 1) v.y += amount;
                if (intensity == 2) v.y *= 1-amount;
                if (intensity == 3) v.y = 1-(1-v.y)*(1-amount);
                color = rgb(v);
                const int effect = number(effectGroup, "ColorEffect", disabled ? 0 : 1);
                const double colorAmount = number(effectGroup, "ColorAmount", disabled ? 0 : -.9);
                if (effect == 1) { v=hcy(color); v.c *= 1-colorAmount; color=rgb(v); }
                if (effect == 2) color=mix(color,effectColor,colorAmount);
                if (effect == 3) color=tint(color,effectColor,colorAmount);
            }
            palette.setColor(state, role.role, color);
        }
        const auto window = palette.color(state, QPalette::Window);
        const auto v = hcy(window);
        const double contrast = qBound(-1., number("KDE", "contrast", 7) * .1, 1.);
        const double light = (.05+v.y*.55)*(.25+contrast*.75);
        const double darkAmount = -v.y*(.55+contrast*.35);
        const QPalette::ColorRole shades[] = {QPalette::Light, QPalette::Midlight, QPalette::Mid, QPalette::Dark, QPalette::Shadow};
        for (int i=0; i<5; ++i) {
            Hcy shade = v;
            if (v.y < .006) {
                const double amounts[] = {.05+.95*contrast, .03+.60*contrast, .01+.20*contrast, .02+.40*contrast, .03+.60*contrast};
                shade.y += amounts[i];
            } else if (v.y > .93) {
                const double amounts[] = {-.04-.40*contrast, -.02-.20*contrast, -.04-.40*contrast, -.06-.60*contrast, -.10-.90*contrast};
                shade.y += amounts[i];
            } else {
                const double amounts[] = {light, (.15+.35*(1-v.y))*light, (.35+.15*v.y)*darkAmount, darkAmount, darkAmount};
                shade.y += amounts[i];
                if (i == 4) shade.y *= 1-(.5+.3*v.y);
            }
            palette.setColor(state, shades[i], rgb(shade));
        }
    }
    QGuiApplication::setPalette(palette);
}
