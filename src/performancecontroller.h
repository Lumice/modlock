#pragma once
#include <QObject>
#include <QTimer>
#include <QVariantMap>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
class PerformanceController final : public QObject {
 Q_OBJECT
 QML_ELEMENT
 QML_UNCREATABLE("Owned by LibraryModel")
 Q_PROPERTY(QVariantList presets READ presets NOTIFY changed)
 Q_PROPERTY(QString selection READ selection NOTIFY changed)
 Q_PROPERTY(QVariantMap detected READ detected NOTIFY changed)
 Q_PROPERTY(bool available READ available NOTIFY changed)
 Q_PROPERTY(bool busy READ busy NOTIFY changed)
 Q_PROPERTY(QString error READ error NOTIFY changed)
public:
 explicit PerformanceController(LibraryModel *library);
 QVariantList presets() const {return m_data.value("presets").toList();}
 QString selection() const {return m_data.value("selection").toString();}
 QVariantMap detected() const {return m_data.value("detected").toMap();}
 bool available() const;
 bool busy() const;
 QString error() const {return m_error;}
 Q_INVOKABLE bool select(const QString &presetId);
 Q_INVOKABLE QVariantList preview(const QString &presetId) const;
 Q_INVOKABLE void refresh();
signals:
 void changed();
private:
 LibraryModel *m_library;
 QVariantMap m_data;
 QString m_error,m_root,m_context;
 bool m_wait=false;
 bool m_retryLaunchBusy=false;
 QTimer m_retryTimer;
};
