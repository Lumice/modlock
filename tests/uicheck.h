#pragma once
#include <QString>
class QQuickWindow;
class LibraryModel;
void startUiCheck(QQuickWindow *window, LibraryModel *model, const QString &scenario);
bool checkGridCapture(QQuickWindow *window, LibraryModel *model, bool updatesObserved);
