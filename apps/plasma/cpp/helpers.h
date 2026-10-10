// Small pieces of Qt that cxx-qt-lib does not wrap.
#pragma once

#include <QtCore/QString>
#include <QtGui/QClipboard>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtQuickControls2/QQuickStyle>

namespace odm {

// Downloads keep running with every window closed (the tray icon brings the
// app back), so closing the last window must not quit.
inline void keepRunningWithoutWindows()
{
    QGuiApplication::setQuitOnLastWindowClosed(false);
}

// The Plasma style, unless the user chose another one.
inline void useDesktopStyle()
{
    if (qEnvironmentVariableIsEmpty("QT_QUICK_CONTROLS_STYLE")) {
        QQuickStyle::setStyle(QStringLiteral("org.kde.desktop"));
    }
}

inline void setWindowIcon(const QString& name)
{
    QGuiApplication::setWindowIcon(QIcon::fromTheme(name));
}

inline void copyToClipboard(const QString& text)
{
    QGuiApplication::clipboard()->setText(text);
}

inline QString clipboardText()
{
    return QGuiApplication::clipboard()->text();
}

} // namespace odm
