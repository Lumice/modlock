#include "networktransport.h"
#include "remotetransfer.h"
#include <QCoreApplication>
#include <csignal>

int main(int argc, char **argv) {
    std::signal(SIGPIPE, SIG_IGN);
    QCoreApplication application(argc, argv);
    const auto args = application.arguments();
    if (args.size() == 2 && args[1] == QStringLiteral("--transfer")) {
        RemoteTransfer transfer;
        return application.exec();
    }
    if ((args.size() != 3 && args.size() != 5) || args[1] != QStringLiteral("--spool-dir")) return 2;
    const bool remote = args.size() == 5;
    if (remote && (args[3] != QStringLiteral("--metadata-owner-fd") || args[4] != QStringLiteral("3"))) return 2;
    NetworkTransport transport(args[2], nullptr, remote ? 3 : -1);
    if (!transport.valid()) return 3;
    return application.exec();
}
