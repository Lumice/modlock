#pragma once
#include <QObject>
#include <QPointer>
#include <QVariantMap>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
class QProcess;
class StorageController : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(LibraryModel* library READ library WRITE setLibrary NOTIFY changed)
    Q_PROPERTY(QVariantMap result READ result NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    
    Q_PROPERTY(double progress READ progress NOTIFY changed)
    Q_PROPERTY(QString phase READ phase NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
public:
    explicit StorageController(QObject *parent=nullptr):QObject(parent){}
    LibraryModel *library() const;
    void setLibrary(LibraryModel *library);
    QVariantMap result() const {return m_result;}
    QString error() const;
    Q_PROPERTY(QString notice READ notice NOTIFY changed)
    QString notice() const {return m_notice;}

    double progress() const {return m_progress;}
    QString phase() const {return m_phase;}
    bool busy() const {return m_busy;}
    Q_INVOKABLE void scan();
    Q_INVOKABLE void review(const QString &folder);
    Q_INVOKABLE void move();
    Q_INVOKABLE void resume();
    Q_INVOKABLE void cancel();
signals:
    void changed();
private:
    void queueScan();
    void request(const QString &method, const QString &folder = {});
    QPointer<LibraryModel> m_library;
    QPointer<QProcess> m_statusProcess;
    QVariantMap m_result;
    QString m_error, m_statusError, m_phase, m_notice;
    quint64 m_epoch=0;
    double m_progress=-1;

    bool m_busy=false, m_rescan=false;
};
