#pragma once
#include "backendclient.h"
#include <QUrl>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
class OptionController final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by LibraryModel")
    Q_PROPERTY(bool visible READ visible NOTIFY changed)
    Q_PROPERTY(bool orderVisible READ orderVisible NOTIFY changed)
    Q_PROPERTY(bool historyVisible READ historyVisible NOTIFY changed)
    Q_PROPERTY(bool available READ available NOTIFY changed)
    Q_PROPERTY(bool editable READ editable NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
    Q_PROPERTY(bool canPrepare READ canPrepare NOTIFY changed)
    Q_PROPERTY(bool canAttach READ canAttach NOTIFY changed)
    Q_PROPERTY(bool canBind READ canBind NOTIFY changed)
    Q_PROPERTY(bool canSave READ canSave NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(QString modId READ modId NOTIFY changed)
    Q_PROPERTY(QString targetName READ targetName NOTIFY changed)
    Q_PROPERTY(QString profileName READ profileName NOTIFY changed)
    Q_PROPERTY(QString profileId READ profileId NOTIFY changed)
    Q_PROPERTY(QVariantList rows READ rows NOTIFY changed)
    Q_PROPERTY(QStringList selectedIds READ selectedIds NOTIFY changed)
    Q_PROPERTY(QVariantMap totals READ totals NOTIFY changed)
    Q_PROPERTY(QVariantMap job READ job NOTIFY changed)
    Q_PROPERTY(QVariantList candidates READ candidates NOTIFY changed)
    Q_PROPERTY(QVariantList bindings READ bindings NOTIFY changed)
    Q_PROPERTY(QVariantList jobs READ jobs NOTIFY changed)
    Q_PROPERTY(QVariantList order READ order NOTIFY changed)
    Q_PROPERTY(QVariantMap attachmentReview READ attachmentReview NOTIFY changed)
    Q_PROPERTY(QVariantMap selectionReview READ selectionReview NOTIFY changed)
public:
    explicit OptionController(LibraryModel *library);
    bool visible()const{return m_visible;} bool orderVisible()const{return m_orderVisible;} bool historyVisible()const{return m_historyVisible;}
    bool hasPreparation()const;
    bool available()const; bool editable()const; bool busy()const; bool canPrepare()const; bool canAttach()const;bool canBind()const;bool canSave()const;
    QString error()const{return m_error;}QString modId()const{return m_mod;}
    QString targetName()const{return m_title;}
    QString profileName()const{return m_profileName;}QString profileId()const{return m_profile;}
    QVariantList rows()const{return m_rows;}QStringList selectedIds()const{return m_selected;}QVariantMap totals()const{return m_totals;}
    QVariantMap job()const;QVariantList candidates()const;QVariantList bindings()const;QVariantList jobs()const;QVariantList order()const{return m_order;}
    QVariantMap attachmentReview()const;QVariantMap selectionReview()const{return m_selectionReview;}
    Q_INVOKABLE bool open(const QString &modId);
    Q_INVOKABLE bool openOrder();
    Q_INVOKABLE void showHistory();
    Q_INVOKABLE void hideHistory();
    Q_INVOKABLE bool showJob(const QString &jobId);
    Q_INVOKABLE bool setSelected(const QString &optionId,bool selected);
    Q_INVOKABLE bool prepareLocal(const QList<QUrl> &roots);
    Q_INVOKABLE bool prepareArchive(const QString &archiveId);
    Q_INVOKABLE bool setBinding(const QString &optionId,const QString &candidateId);
    Q_INVOKABLE bool reviewBindings(const QVariantList &bindings);
    Q_INVOKABLE bool attach();
    Q_INVOKABLE bool previewSelection();
    Q_INVOKABLE bool commitSelection();
    Q_INVOKABLE void discardSelectionReview();
    Q_INVOKABLE void discardAttachmentReview();
    Q_INVOKABLE bool move(const QString &modId,int offset);
    Q_INVOKABLE bool previewOrder();
    Q_INVOKABLE bool cancel();
    Q_INVOKABLE void hide(bool order = false);
    Q_INVOKABLE bool canCancelJob(const QString &jobId) const;
    Q_INVOKABLE bool cancelJob(const QString &jobId);
    Q_INVOKABLE bool dismiss(const QString &jobId);
    void acceptProfileReview(const QVariantMap &review);
    void finished(bool success,const QString &message={},const QString &attachedJob={});
    void rejected(const QString &message);
signals:
    void changed();
private:
    struct Job {QVariantMap status,review;QVariantList candidates,bindings;bool candidatesLoaded=false;};
    void sync();void poll();bool request(BackendClient::Call call,QJsonObject params={});
    void accept(BackendClient::Call call,const QVariantMap &result);
    bool context()const;bool prepare(QJsonObject source);void updateHistory();
    void invalidate(const QString &reason);void pin();void edits();
    LibraryModel *m_library;
    QTimer m_poll;
    QMap<QString,Job> m_jobs;
    QString m_root,m_helper,m_profile,m_revision,m_mod,m_job,m_error,m_refreshJob,m_title,m_profileName;
    QVariantList m_rows,m_order;QStringList m_selected;QVariantMap m_totals,m_selectionReview;
    bool m_visible=false,m_orderVisible=false,m_historyVisible=false,m_wait=false,m_loaded=false,m_committing=false,m_detailPending=false;
    quint64 m_edits=0,m_previewEdits=0;
    QJsonObject m_previewChange;
};
