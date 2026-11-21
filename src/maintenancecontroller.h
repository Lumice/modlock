#pragma once
#include <QObject>
#include <QPointer>
#include <QVariantMap>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
class MaintenanceController : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(LibraryModel* library READ library WRITE setLibrary NOTIFY changed)
    Q_PROPERTY(QVariantMap result READ result NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(QVariant freedBytes READ freedBytes NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
public:
    explicit MaintenanceController(QObject *parent=nullptr):QObject(parent){}
    LibraryModel *library() const;
    void setLibrary(LibraryModel *library);
    QVariantMap result() const {return m_result;}
    QString error() const {return m_error;}
    QVariant freedBytes() const {return m_freedBytes;}
    bool busy() const {return m_busy;}
    Q_INVOKABLE void scan();
    Q_INVOKABLE void clean();
signals:
    void changed();
private:
    void request(bool clean);
    QPointer<LibraryModel> m_library;
    QVariantMap m_result;
    QString m_error;
    QVariant m_freedBytes;
    bool m_busy=false, m_rescan=false;
};
