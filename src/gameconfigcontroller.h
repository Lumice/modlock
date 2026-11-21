#pragma once
#include <QObject>
#include <QVariantMap>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
class GameConfigController final : public QObject {
  Q_OBJECT
  QML_ELEMENT
  QML_UNCREATABLE("Owned by LibraryModel")
  Q_PROPERTY(QString text READ text NOTIFY changed)
  Q_PROPERTY(QString path READ path NOTIFY changed)
  Q_PROPERTY(QVariantList managedSections READ managedSections NOTIFY changed)
  Q_PROPERTY(QVariantMap values READ values NOTIFY changed)
  Q_PROPERTY(QVariantList heroScopes READ heroScopes NOTIFY changed)
  Q_PROPERTY(QString editHero READ editHero WRITE setEditHero NOTIFY changed)
  Q_PROPERTY(QVariantMap defaults READ defaults NOTIFY changed)
  Q_PROPERTY(QVariantMap ranges READ ranges NOTIFY changed)
  Q_PROPERTY(QVariantMap review READ review NOTIFY changed)
  Q_PROPERTY(bool loaded READ loaded NOTIFY changed)
  Q_PROPERTY(bool available READ available NOTIFY changed)
  Q_PROPERTY(bool busy READ busy NOTIFY changed)
  Q_PROPERTY(QString error READ error NOTIFY changed)
  Q_PROPERTY(bool canUndo READ canUndo NOTIFY changed)
public:
  GameConfigController(LibraryModel *, const QString &);
  QString text() const { return m_data.value("text").toString(); }
  QString path() const { return m_data.value("path").toString(); }
  QVariantList managedSections() const {
    return m_data.value("managedSections").toList();
  }
  QVariantMap values() const { return m_values; }
  QVariantList heroScopes() const { return m_data.value("heroScopes").toList(); }
  QString editHero() const { return m_editHero; }
  void setEditHero(const QString &hero);
  QVariantMap savedValues() const { return m_data.value("values").toMap(); }
  QVariantMap defaults() const { return m_data.value("defaults").toMap(); }
  QVariantMap ranges() const { return m_data.value("ranges").toMap(); }
  QVariantMap review() const { return m_review; }
  QString error() const { return m_error; }
  bool loaded() const { return available() && !busy() && m_error.isEmpty() && !m_data.isEmpty(); }
  bool available() const;
  bool busy() const;
  bool canUndo() const { return m_data.value("canUndo").toBool(); }
  Q_INVOKABLE void refresh();
  Q_INVOKABLE bool preview(const QString &text);
  Q_INVOKABLE bool preview();
  Q_INVOKABLE bool commit(const QString &token);
  Q_INVOKABLE bool undo();
  Q_INVOKABLE bool setValue(const QString &key, const QVariant &value);
  Q_INVOKABLE void clearHeroOverride(const QString &hero);
  Q_INVOKABLE bool importCode(const QString &code);
  Q_INVOKABLE QString exportCode() const;
signals:
  void changed();
  void reviewReady(const QVariantMap &review);

private:
  bool request(int call, const QVariantMap &params = {});
  LibraryModel *m_library;
  QString m_kind, m_error, m_context, m_editHero, m_codeHero;
  QVariantMap m_data, m_values, m_review;
  bool m_wait = false, m_sending = false;
};
