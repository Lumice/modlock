#pragma once
#include <QString>
#include <QVariant>
#include <algorithm>

namespace ApplyWarningPolicy {
inline bool onlyOverlapNotes(const QVariantList &warnings) {
    for (const auto &warning : warnings) {
        // Local-import warnings use a different schema. Never reinterpret a
        // hybrid or loosely converted row as canonical Apply evidence.
        if (warning.metaType().id() != QMetaType::QVariantMap) return false;
        const auto row = warning.toMap();
        if (row.size() != 2 || row.value("code").metaType().id() != QMetaType::QString
                || row.value("message").metaType().id() != QMetaType::QString) return false;
        const auto message = row.value("message").toString();
        if (message.isEmpty() || std::any_of(message.cbegin(), message.cend(), [](QChar c) { return c.category() == QChar::Other_Control; }) || message.toUtf8().size() > 16384) return false;
        const auto code = row.value("code").toString();
        if (code != QLatin1String("fileConflict") && code != QLatin1String("additionalConflicts")) return false;
    }
    return true;
}
}
