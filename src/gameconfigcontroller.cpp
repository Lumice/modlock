#include "gameconfigcontroller.h"
#include "backendclient.h"
#include "librarymodel.h"
#include <QJsonObject>
#include <QUrl>
#include <cmath>
GameConfigController::GameConfigController(LibraryModel *l, const QString &kind)
    : QObject(l), m_library(l), m_kind(kind) {
  connect(l, &LibraryModel::nativeChanged, this, [this] {
    const auto c = m_library->dataDir() + "|" + m_library->revision() + "|" +
                   m_library->gamePath() + "|" +
                   QString::number(m_library->m_client->sessionGeneration());
    if (c != m_context && !busy() && available()) {
      m_context = c;
      m_review.clear();
      m_data.clear();
      m_values.clear();
      refresh();
    }
    emit changed();
  });
  connect(l->m_client, &BackendClient::capabilitiesReady, this, [this] {
    m_wait = false;
    m_context.clear();
    refresh();
  });
  connect(l->m_client, &BackendClient::configReady, this,
          [this](BackendClient::Call call, const QVariantMap &data) {
            if (data.value("kind").toString() != m_kind)
              return;
            m_wait = false;
            m_error.clear();
            if (call == BackendClient::Call::ConfigPreview) {
              m_review = data;
              emit reviewReady(data);
            } else if (call == BackendClient::Call::ConfigDecode) {
              // Apply to the scope selected when the request began. Legacy
              // codes carry no newer settings, so retain those in that scope.
              auto all = m_values.value("overrides").toMap();
              auto values = m_codeHero.isEmpty() ? m_values : all.value(m_codeHero).toMap();
              for (const auto *key : {"gap", "width", "height", "pipOpacity", "pipBorder",
                                      "pipGapStatic", "dotOpacity", "dotOutlineOpacity", "color"})
                values[key] = data.value(key);
              values["pipOutlineWidth"] = data.value("pipBorder").toBool() ? 1 : 0;
              if (m_codeHero.isEmpty()) { values["hero"] = data.value("hero"); m_values = values; }
              else { all[m_codeHero] = values; m_values["overrides"] = all; }
              m_review.clear();
            } else {
              m_data = data;
              m_values = data.value("values").toMap();
              m_review.clear();
            }
            emit changed();
          });
  connect(l->m_client, &BackendClient::rejected, this,
          [this](BackendClient::Call c, const QString &, const QString &e) {
            if (m_sending && int(c) >= 181 && int(c) <= 184) {
              m_wait = false;
              m_error = e;
              m_review.clear();
              emit changed();
            }
          });
  connect(l->m_client, &BackendClient::configRejected, this,
          [this](const QString &kind, const QString &error) {
            if (kind != m_kind)
              return;
            m_wait = false;
            m_error = error;
            m_review.clear();
            emit changed();
          });
  connect(l->m_client, &BackendClient::failed, this, [this](const QString &e) {
    m_wait = false;
    m_error = e;
    m_review.clear();
    emit changed();
  });
}
bool GameConfigController::available() const {
  return m_library->nativeMode() && !m_library->gamePath().isEmpty() &&
         m_library->m_client->readyForRequests();
}
bool GameConfigController::busy() const {
  return m_wait || m_library->busy() || m_library->loading();
}
bool GameConfigController::request(int c, const QVariantMap &p) {
  if (!available() || busy())
    return false;
  // A Play attempt owns the game setup until the library has shown its result.
  if ((c == 182 || c == 183) && m_library->launch()->holdsSetup()) {
    m_error = tr("A launch owns the game setup. Check launch status before editing.");
    emit changed();
    return false;
  }
  auto params = p;
  params.insert("kind", m_kind);
  m_wait = true;
  m_sending = true;
  m_library->m_client->nativeRequest(BackendClient::Call(c),
                                     QJsonObject::fromVariantMap(params));
  m_sending = false;
  emit changed();
  return true;
}
void GameConfigController::setEditHero(const QString &hero) {
  if (m_editHero == hero) return;
  m_editHero = hero; emit changed();
}
void GameConfigController::refresh() { request(181); }
bool GameConfigController::preview(const QString &s) {
  m_review.clear();
  return m_kind == "autoexec" && request(182, {{"input", s}});
}
bool GameConfigController::preview() {
  m_review.clear();
  return m_kind == "crosshair" && request(182, {{"input", m_values}});
}
bool GameConfigController::commit(const QString &t) {
  if (t.isEmpty() || t != m_review.value("token").toString())
    return false;
  return request(183, {{"token", t}});
}
bool GameConfigController::undo() {
  m_review.clear();
  return canUndo() && request(182, {{"undo", true}});
}
bool GameConfigController::setValue(const QString &path, const QVariant &value) {
  if (busy() || m_kind != "crosshair") return false;
  const auto parts = path.split('.');
  QString key = path, hero;
  auto all = m_values.value("overrides").toMap();
  QVariantMap values = m_values;
  if (parts.size() >= 3 && parts[0] == "overrides") {
    hero = parts[1];
    if (!hero.startsWith("hero_")) return false;
    for (const auto c : hero)
      if (c != '_' && !(c >= 'a' && c <= 'z') && !(c >= '0' && c <= '9')) return false;
    key = parts.mid(2).join('.');
    if (key == "hero") return false;
    values = all.value(hero).toMap();
  }
  if (key == "pipBorder" || key == "pipGapStatic" || key == "gapCollapse" || key == "themed") {
    if (value.metaType().id() != QMetaType::Bool) return false;
    values[key] = value;
  } else if (key == "hero") {
    if (!m_data.value("heroes").toStringList().contains(value.toString())) return false;
    values[key] = value;
  } else {
    const auto r = ranges().value(key).toMap();
    bool ok;
    const double n = value.toDouble(&ok), step = r.value("step").toDouble();
    if (r.isEmpty() || !ok || !std::isfinite(n) || n < r.value("min").toDouble() ||
        n > r.value("max").toDouble() || std::abs(n / step - std::round(n / step)) > 1e-6) return false;
    if (key.startsWith("color.") || key.startsWith("outlineColor.")) {
      const QString group = key.section('.',0,0);
      auto color = values.value(group).toMap();
      color[key.section('.',1,1)] = n;
      values[group] = color;
    } else values[key] = n;
  }
  if (hero.isEmpty()) m_values = values;
  else { all[hero] = values; m_values["overrides"] = all; }
  m_review.clear();
  emit changed();
  return true;
}
void GameConfigController::clearHeroOverride(const QString &hero) {
  if (busy() || m_kind != "crosshair") return;
  auto all = m_values.value("overrides").toMap();
  all.remove(hero);
  m_values["overrides"] = all;
  m_review.clear(); emit changed();
}
bool GameConfigController::importCode(const QString &s) {
  if (m_kind != "crosshair" || busy()) return false;
  m_codeHero = m_editHero;
  return request(184, {{"code", s}});
}
QString GameConfigController::exportCode() const {
  if (m_values.isEmpty())
    return {};
  QStringList v;
  for (const auto *k : {"gap", "width", "height", "pipOpacity", "pipBorder",
                        "pipGapStatic", "dotOpacity", "dotOutlineOpacity"})
    v << (QString(k) == "pipBorder" ? (m_values.value("pipOutlineWidth").toDouble() > 0 ? "true" : "false") : m_values.value(k).toString());
  auto color = m_values.value("color").toMap();
  for (const auto *k : {"r", "g", "b"})
    v << color.value(k).toString();
  v << QString::fromLatin1(
      QUrl::toPercentEncoding(m_values.value("hero").toString()));
  return v.join(';');
}
