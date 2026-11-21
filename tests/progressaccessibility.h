#pragma once
#include <QAccessible>
#include <QObject>
#include <QString>
#include <QVariant>
#include <QtMath>

inline bool progressAccessibilityMatches(QObject *control, QString &error) {
    auto *interface = control ? QAccessible::queryAccessibleInterface(control) : nullptr;
    if (!interface || interface->role() != QAccessible::ProgressBar || !interface->valueInterface()) {
        error = QStringLiteral("Download progress has no accessible progress role/value interface");
        return false;
    }
    auto *value = interface->valueInterface();
    const bool matches = qAbs(value->minimumValue().toDouble() - control->property("from").toDouble()) < 0.0001
        && qAbs(value->maximumValue().toDouble() - control->property("to").toDouble()) < 0.0001
        && qAbs(value->currentValue().toDouble() - control->property("value").toDouble()) < 0.0001;
    if (!matches) error = QStringLiteral("Accessible progress range/value differs from the native control");
    return matches;
}
