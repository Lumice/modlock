#pragma once
#include <QObject>
#include <QString>
#include <QTextBoundaryFinder>
#include <QtQml/qqmlregistration.h>

class TextUtils : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON
public:
    using QObject::QObject;
    Q_INVOKABLE QString initial(const QString &name) const {
        const QString text = name.trimmed();
        if (text.isEmpty()) return QStringLiteral("P");
        QTextBoundaryFinder boundary(QTextBoundaryFinder::Grapheme, text);
        return text.left(boundary.toNextBoundary()).toUpper();
    }
};
