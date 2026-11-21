#include "instancebroker.h"
#include <QCryptographicHash>
#include <QDir>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocalSocket>
#include <QStandardPaths>
#include <QTimer>
#include <unistd.h>
InstanceBroker::Result InstanceBroker::start(const QString &library,const QStringList &links) {
    if(links.size()>10) return Failed;
    const auto message=QJsonDocument(QJsonArray::fromStringList(links)).toJson(QJsonDocument::Compact)+'\n';
    if(message.size()>16384)return Failed;
    const auto directory=QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation)+"/deadlock-native";
    if(!QDir().mkpath(directory))return Failed;
    const QFileInfo info(directory);
    if(info.isSymLink() || info.ownerId()!=getuid() || !QFile::setPermissions(directory,QFile::ReadOwner|QFile::WriteOwner|QFile::ExeOwner))return Failed;
    const auto socket=directory+"/"+QString::fromLatin1(QCryptographicHash::hash(library.toUtf8(),QCryptographicHash::Sha256).toHex().left(24));
    m_lock=std::make_unique<QLockFile>(socket+".lock");
    if(!m_lock->tryLock(0)) {
        for(int attempt=0;attempt<20;++attempt) {
            QLocalSocket peer;peer.connectToServer(socket);
            if(peer.waitForConnected(100)) {
                peer.write(message);if(!peer.waitForBytesWritten(1000))return Failed;
                return peer.waitForReadyRead(1000) && peer.readAll()=="ok\n" ? Forwarded : Failed;
            }
            // The lock winner may still be creating its listener.
            usleep(50000);
        }
        return Failed;
    }
    QLocalServer::removeServer(socket);
    m_server.setSocketOptions(QLocalServer::UserAccessOption);
    if(!m_server.listen(socket))return Failed;
    connect(&m_server,&QLocalServer::newConnection,this,[this]{
        while(auto *peer=m_server.nextPendingConnection()) {
            peer->setReadBufferSize(16385);
            auto bytes=std::make_shared<QByteArray>();
            auto done=std::make_shared<bool>(false);
            connect(peer,&QLocalSocket::disconnected,peer,&QObject::deleteLater);
            QTimer::singleShot(2000,peer,[peer]{peer->abort();peer->deleteLater();});
            auto read=[this,peer,bytes,done]{
                if(*done)return;
                bytes->append(peer->readAll());
                if(bytes->size()>16384){*done=true;peer->abort();return;}
                if(!bytes->contains('\n'))return;
                *done=true;
                const auto doc=QJsonDocument::fromJson(*bytes);
                if(!doc.isArray() || doc.array().size()>10){peer->abort();return;}
                QStringList urls;
                for(const auto &v:doc.array()){if(!v.isString() || v.toString().size()>8192){peer->abort();return;}urls.append(v.toString());}
                emit received(urls);peer->write("ok\n");peer->disconnectFromServer();
            };
            connect(peer,&QLocalSocket::readyRead,peer,read);read();
        }
    });
    return Primary;
}
