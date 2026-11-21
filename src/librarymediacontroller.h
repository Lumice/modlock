#pragma once

#include "catalogdata.h"
#include <QObject>
#include <QHash>
#include <QSet>
#include <optional>
#include <functional>

class CatalogController;

struct LibraryMediaProjection {
    QStringList images;
    bool masked = true;
    QString authorId;
    bool operator==(const LibraryMediaProjection &) const = default;
};

class LibraryMediaController final : public QObject {
    Q_OBJECT
public:
    explicit LibraryMediaController(CatalogController *, QObject *parent = nullptr);
    void setContext(const QString &root, quint64 epoch, bool active);
    void demand(const QString &consumer, const QString &rowId, const SubmissionKey &submission, bool wanted);
    void clear();
    void retainRows(const std::function<std::optional<SubmissionKey>(const QString &)> &current);
    std::optional<LibraryMediaProjection> projection(const QString &rowId, const SubmissionKey &submission) const;
signals:
    void mediaChanged(const QString &rowId);
    void capacityAvailable();
private:
    struct Demand { QString row, identity; SubmissionKey submission; };
    struct Facts { bool readAttempted=false, cacheHasMedia=false, refreshWanted=false, refreshAttempted=false; };
    struct Entry { Demand demand; LibraryMediaProjection media; qsizetype bytes=0; quint64 used=0; };
    struct Work { Demand demand; QString key, session; bool refresh=false, admitted=false; };
    static QString identity(const QString &row, const SubmissionKey &submission);
    void schedule();
    void notifyCapacity();
    void pump();
    void catalogChanged();
    void receive(const QString &key, const QString &root, const CatalogDecodedResult &result);
    void observe(const QString &root, const CatalogDecodedResult &result);
    void invalidate(const QString &root, const QString &session, const QString &submission);
    void withdraw(const QString &consumer);
    void cancelWork();
    void clearEntries();
    void removeEntry(const QString &identity);
    void putEntry(const Demand &, const LibraryMediaProjection &);
    void removeSubmission(const SubmissionKey &);
    void updateSubmission(const Demand &, const LibraryMediaProjection &);
    bool needsRead(const Demand &) const;
    bool needsRefresh(const Demand &) const;
    CatalogController *m_catalog;
    QString m_root, m_session;
    quint64 m_epoch=0, m_clock=0;
    bool m_active=false, m_scheduled=false, m_capacityScheduled=false;
    QHash<QString,Demand> m_consumers, m_demands;
    QList<QString> m_order;
    QSet<QString> m_satisfied;
    QHash<QString,Facts> m_facts;
    QHash<QString,Entry> m_entries;
    qsizetype m_entryBytes=0;
    std::optional<Work> m_work;
};
