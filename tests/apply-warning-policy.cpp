#include "applywarningpolicy.h"
#include <QList>
#include <QPair>
#include <cstdio>

int main() {
    const QVariantMap overlap{{"code", "fileConflict"}, {"message", "First mod wins."}};
    const QVariantMap summary{{"code", "additionalConflicts"}, {"message", "More overlaps follow the same order."}};
    const QVariantMap realWarning{{"code", "blacklisted"}, {"message", "Needs review."}};
    const QVariantMap hybrid{{"kind", "blacklisted"}, {"code", "fileConflict"}, {"message", "Needs review."}};
    const QList<QPair<QVariantList, bool>> cases{
        {{}, true}, {{overlap}, true}, {{summary}, true}, {{overlap, summary}, true},
        {{realWarning}, false}, {{overlap, realWarning}, false}, {{hybrid}, false},
        {{QVariantMap{{"code", "fileConflict"}}}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", 12}}}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", ""}}}, false},
        {{QVariantMap{{"message", "No code."}}}, false}, {{QStringLiteral("fileConflict")}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", QString(16385, 'x')}}}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", "\n"}}}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", "Wins\torder"}}}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", QString(QChar(0x7f))}}}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", QString(QChar(0))}}}, false},
        {{QVariantMap{{"code", "fileConflict"}, {"message", QString(16384, 'x')}}}, true},
        {{QVariantMap{{"code", "fileConflict"}, {"message", QString(8193, QChar(0xe9))}}}, false}
    };
    for (qsizetype index=0; index<cases.size(); ++index) {
        if (ApplyWarningPolicy::onlyOverlapNotes(cases[index].first) != cases[index].second) {
            std::fprintf(stderr, "FAIL automatic Apply warning policy case %lld\n", static_cast<long long>(index));
            return 1;
        }
    }
    std::puts("PASS automatic Apply warning shape and informational codes");
}
