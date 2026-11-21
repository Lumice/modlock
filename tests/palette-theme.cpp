// SPDX-License-Identifier: GPL-3.0-or-later
#include <qpa/qplatformthemeplugin.h>
#include <qpa/qplatformtheme.h>
#include <QPalette>
class PaletteTheme final : public QPlatformTheme {
    QPalette colors{QColor("#202326")};
public:
    PaletteTheme() { colors.setColor(QPalette::WindowText, QColor("#bedcf0")); }
    const QPalette *palette(Palette type) const override {
        return type == SystemPalette ? &colors : nullptr;
    }
};
class NoPaletteTheme final : public QPlatformTheme {
public:
    const QPalette *palette(Palette) const override { return nullptr; }
};
class PaletteThemePlugin final : public QPlatformThemePlugin {
    Q_OBJECT
    Q_PLUGIN_METADATA(IID QPlatformThemeFactoryInterface_iid FILE "palette-theme.json")
public:
    QPlatformTheme *create(const QString &key, const QStringList &) override {
        return key == "palette-none" ? static_cast<QPlatformTheme *>(new NoPaletteTheme) : new PaletteTheme;
    }
};
#include "palette-theme.moc"
