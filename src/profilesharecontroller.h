#pragma once
#include <QObject>
#include <QPointer>
#include <QVariantMap>
#include <QUrl>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
class ProfileShareController : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(LibraryModel* library READ library WRITE setLibrary NOTIFY changed)
    Q_PROPERTY(QVariantMap result READ result NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
public:
    explicit ProfileShareController(QObject *parent = nullptr) : QObject(parent) {}
    LibraryModel *library() const;
    void setLibrary(LibraryModel *library);
    QVariantMap result() const { return m_result; }
    QString error() const { return m_error; }
    bool busy() const { return m_busy; }
    Q_INVOKABLE void exportProfile(const QString &id, bool includeNames);
    Q_INVOKABLE void review(const QString &code);
    Q_INVOKABLE void create(const QString &code, const QString &name);
    Q_INVOKABLE void copyCode();
    Q_INVOKABLE QString readFile(const QUrl &url);
    Q_INVOKABLE bool download(const QVariantMap &submission, const QString &fileId);
    Q_INVOKABLE void invalidate();
signals:
    void changed();
    void created();
private:
    void request(const QString &action, QVariantMap params);
    QPointer<LibraryModel> m_library;
    QVariantMap m_result;
    QString m_error, m_reviewCode, m_reviewRevision;
    bool m_busy = false;
};
