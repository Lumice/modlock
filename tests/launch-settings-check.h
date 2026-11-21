// Synthetic launch arguments only. No helper opens a real library or game.
struct LaunchSettingsPeer {
    QTemporaryDir temporary;
    QString root = temporary.filePath("library");
    QString helper = QStringLiteral(TEST_SOURCE_DIR) + "/tests/launch-settings-peer.py";
    explicit LaunchSettingsPeer(const QString &mode = "ordinary") {
        QVERIFY(temporary.isValid()); QVERIFY(QDir().mkpath(root)); put("mode",mode.toUtf8());
    }
    void put(const QString &name,const QByteArray &bytes = {}) const {
        QFile file(QDir(root).filePath(name)); QVERIFY(file.open(QIODevice::WriteOnly));
        QCOMPARE(file.write(bytes),bytes.size());
    }
    bool has(const QString &name) const { return QFile::exists(QDir(root).filePath(name)); }
    QList<QJsonObject> calls(const QString &method) const {
        QFile file(QDir(root).filePath("calls.jsonl")); QList<QJsonObject> out;
        if(file.open(QIODevice::ReadOnly)) for(const auto &line:file.readAll().split('\n')) {
            const auto value=QJsonDocument::fromJson(line).object();
            if(value["method"]==QJsonValue(method))out.append(value);
        }
        return out;
    }
};
