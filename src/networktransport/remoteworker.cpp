#include "remoteworker.h"
#include <QByteArrayView>
#include <QMetaObject>
#include <algorithm>
#include <cerrno>
#include <chrono>
#include <fcntl.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>

namespace { constexpr qsizetype QueueLimit=4*1024*1024; constexpr qsizetype ChunkLimit=64*1024; }
RemoteTransferWorker::RemoteTransferWorker(QObject *receiver,std::function<void()> wake,quint64 offset,quint64 device,quint64 inode,quint64 uid)
    : receiver_(receiver),wake_(std::move(wake)),initial_(offset),device_(device),inode_(inode),uid_(uid) {
    state_.written=offset;thread_=std::thread([this]{run();});
}
RemoteTransferWorker::~RemoteTransferWorker() {stop();if(thread_.joinable())thread_.join();}
void RemoteTransferWorker::notifyLocked() {
    if(notified_)return;notified_=true;
    QMetaObject::invokeMethod(receiver_,[this]{
        {std::lock_guard lock(mutex_);notified_=false;}
        wake_();
    },Qt::QueuedConnection);
}
bool RemoteTransferWorker::enqueue(QByteArray bytes) {
    if(bytes.isEmpty()||bytes.size()>ChunkLimit)return false;
    std::lock_guard lock(mutex_);
    if(stop_||finish_||checkpoint_||waitingAck_||!state_.ready||state_.failed||state_.charged>QueueLimit-bytes.size())return false;
    state_.charged+=bytes.size();queue_.push_back(std::move(bytes));condition_.notify_one();return true;
}
void RemoteTransferWorker::checkpoint() {std::lock_guard lock(mutex_);checkpoint_=true;condition_.notify_one();}
void RemoteTransferWorker::acknowledge() {std::lock_guard lock(mutex_);waitingAck_=false;state_.checkpointReady=false;state_.checkpointHash.clear();condition_.notify_one();}
void RemoteTransferWorker::finish() {std::lock_guard lock(mutex_);finish_=true;condition_.notify_one();}
void RemoteTransferWorker::stop() {std::lock_guard lock(mutex_);stop_=true;condition_.notify_one();}
RemoteTransferWorker::State RemoteTransferWorker::state() {std::lock_guard lock(mutex_);return state_;}
bool RemoteTransferWorker::validateDescriptors() {
    struct stat w{},r{},l{};if(fstat(3,&w)<0||fstat(4,&r)<0||fstat(5,&l)<0)return false;
    const int wf=fcntl(3,F_GETFL),rf=fcntl(4,F_GETFL);
    return S_ISREG(w.st_mode)&&S_ISREG(r.st_mode)&&S_ISREG(l.st_mode)
        &&quint64(w.st_dev)==device_&&quint64(w.st_ino)==inode_&&quint64(w.st_uid)==uid_&&uid_==geteuid()
        &&w.st_dev==r.st_dev&&w.st_ino==r.st_ino&&w.st_uid==r.st_uid&&w.st_nlink==1&&r.st_nlink==1
        &&l.st_uid==uid_&&l.st_nlink==1&&(w.st_mode&0077)==0&&(l.st_mode&0077)==0
        &&(w.st_dev!=l.st_dev||w.st_ino!=l.st_ino)&&w.st_size>=0&&quint64(w.st_size)==initial_
        &&wf>=0&&(wf&O_ACCMODE)!=O_RDONLY&&!(wf&O_APPEND)&&rf>=0&&(rf&O_ACCMODE)==O_RDONLY
        &&flock(5,LOCK_SH|LOCK_NB)==0;
}
bool RemoteTransferWorker::readPrefix(QCryptographicHash &hash,quint64 bytes) {
    char buffer[64*1024];quint64 at=0;struct stat before{};
    auto lastActivity=std::chrono::steady_clock::now();
    if(fstat(4,&before)<0||before.st_size<0||quint64(before.st_size)!=bytes)return false;
    while(at<bytes) {
        const size_t amount=size_t(std::min<quint64>(sizeof(buffer),bytes-at));
        const ssize_t n=pread(4,buffer,amount,off_t(at));
        if(n<0&&errno==EINTR)continue;if(n<=0)return false;
        hash.addData(QByteArrayView(buffer,n));at+=quint64(n);
        const auto now=std::chrono::steady_clock::now();
        {std::lock_guard lock(mutex_);if(stop_)return false;if(now-lastActivity>=std::chrono::seconds(1)){++state_.activity;notifyLocked();lastActivity=now;}}
    }
    struct stat after{};return fstat(4,&after)==0 && after.st_dev==before.st_dev && after.st_ino==before.st_ino && after.st_size==before.st_size;
}
bool RemoteTransferWorker::finalHash(quint64 bytes,const QByteArray &expected,QByteArray &md5,QByteArray &sha) {
    struct stat before{};if(fstat(4,&before)<0||before.st_size<0||quint64(before.st_size)!=bytes)return false;
    QCryptographicHash m(QCryptographicHash::Md5),s(QCryptographicHash::Sha256);char buffer[64*1024];quint64 at=0;
    auto lastActivity=std::chrono::steady_clock::now();
    while(at<bytes) {
        const size_t amount=size_t(std::min<quint64>(sizeof(buffer),bytes-at));const ssize_t n=pread(4,buffer,amount,off_t(at));
        if(n<0&&errno==EINTR)continue;if(n<=0)return false;m.addData(QByteArrayView(buffer,n));s.addData(QByteArrayView(buffer,n));at+=quint64(n);
        const auto now=std::chrono::steady_clock::now();
        {std::lock_guard lock(mutex_);if(stop_)return false;if(now-lastActivity>=std::chrono::seconds(1)){++state_.activity;notifyLocked();lastActivity=now;}}
    }
    struct stat after{};if(fstat(4,&after)<0||after.st_dev!=before.st_dev||after.st_ino!=before.st_ino||after.st_size!=before.st_size)return false;
    sha=s.result().toHex();md5=m.result().toHex();return sha==expected;
}
void RemoteTransferWorker::run() {
    QCryptographicHash hash(QCryptographicHash::Sha256);
    const bool identity=validateDescriptors();
    bool good=identity&&readPrefix(hash,initial_);
    bool writeOpen=true;
    {
        std::lock_guard lock(mutex_);if(!good&&!stop_)state_.failed=true;state_.identityFailed=!identity;state_.ready=good;notifyLocked();
    }
    while(good) {
        QByteArray chunk;bool doCheckpoint=false,doFinish=false;
        {
            std::unique_lock lock(mutex_);
            condition_.wait(lock,[this]{return stop_||(!waitingAck_&&(!queue_.empty()||checkpoint_||finish_));});
            if(stop_)break;
            if(!queue_.empty()){chunk=std::move(queue_.front());queue_.pop_front();}
            else if(checkpoint_){checkpoint_=false;doCheckpoint=true;}
            else if(finish_)doFinish=true;
        }
        if(!chunk.isEmpty()) {
            qsizetype done=0;
            while(done<chunk.size()) {
                quint64 position;{std::lock_guard lock(mutex_);position=state_.written;}
                const ssize_t n=pwrite(3,chunk.constData()+done,size_t(chunk.size()-done),off_t(position));
                if(n<0&&errno==EINTR)continue;if(n<=0){good=false;break;}
                hash.addData(QByteArrayView(chunk.constData()+done,n));done+=n;
                {std::lock_guard lock(mutex_);state_.written+=quint64(n);++state_.activity;notifyLocked();}
            }
            {std::lock_guard lock(mutex_);state_.charged-=chunk.size();if(!good)state_.failed=true;notifyLocked();}
        } else if(doCheckpoint) {
            if(fsync(3)<0){good=false;std::lock_guard lock(mutex_);state_.failed=true;notifyLocked();continue;}
            std::lock_guard lock(mutex_);state_.checkpointHash=hash.result().toHex();state_.checkpointReady=true;waitingAck_=true;++state_.activity;notifyLocked();
        } else if(doFinish) {
            if(fsync(3)<0){good=false;std::lock_guard lock(mutex_);state_.failed=true;notifyLocked();break;}
            const int closed=close(3);writeOpen=false;
            if(closed<0){good=false;std::lock_guard lock(mutex_);state_.failed=true;notifyLocked();break;}
            quint64 total;{std::lock_guard lock(mutex_);total=state_.written;}
            QByteArray md5,sha;good=finalHash(total,hash.result().toHex(),md5,sha);
            {std::lock_guard lock(mutex_);state_.failed=!good&&!stop_;state_.md5=md5;state_.sha256=sha;state_.done=true;notifyLocked();}
            close(4);return;
        }
    }
    if(writeOpen)close(3);close(4);
    {std::lock_guard lock(mutex_);queue_.clear();state_.charged=0;state_.done=true;notifyLocked();}
}
