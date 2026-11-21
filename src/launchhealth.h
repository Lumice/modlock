#pragma once
#include <QObject>
#include <QVariantMap>
#include <QElapsedTimer>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
// Five-minute attribution window follows upstream DMM (GPL-3.0).
class LaunchHealth final : public QObject {
 Q_OBJECT
 QML_ELEMENT
 QML_UNCREATABLE("Owned by LaunchController")
 Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY changed)
 Q_PROPERTY(QVariantMap report READ report NOTIFY changed)
 Q_PROPERTY(QString error READ error NOTIFY changed)
public:
 explicit LaunchHealth(LibraryModel *model);
 bool enabled() const { return m_enabled; }
 void setEnabled(bool enabled);
 QVariantMap report() const { return m_report; }
 QString error() const { return m_error; }
 void load(const QString &root);
 void capture(const QString &attempt, const QString &mode);
 void observe(const QString &state);
 void stopped() { m_finished = true; }
 static QString classify(qint64 seconds, bool observed, bool abnormal = false, bool stopped = false);
 static QVariantList differences(const QVariantMap &before, const QVariantMap &after);
 Q_INVOKABLE void answer(bool crashed);
 Q_INVOKABLE void dismiss();
 Q_INVOKABLE bool changeBisectAnswer();
 Q_INVOKABLE bool undo(int index);
signals:
 void changed();
 void configReviewRequested(const QString &kind);
private:
 QVariantMap snapshot() const;
 void save();
 LibraryModel *m_model;
 QString m_root,m_file,m_attempt,m_error;
 QVariantMap m_good,m_launch,m_report;
 QElapsedTimer m_uptime;
 bool m_enabled=true,m_seen=false,m_finished=false;
};
