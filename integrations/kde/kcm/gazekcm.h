// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#pragma once

#include <KQuickConfigModule>
#include <QDBusMessage>
#include <functional>

#include "facemodel.h"

class GazeKcm : public KQuickConfigModule
{
    Q_OBJECT
    Q_PROPERTY(FaceModel *faces READ faces CONSTANT)
    Q_PROPERTY(QString user READ user CONSTANT)
    Q_PROPERTY(QString error READ error NOTIFY errorChanged)

public:
    GazeKcm(QObject *parent, const KPluginMetaData &data);

    FaceModel *faces() { return &m_faces; }
    QString user() const { return m_user; }
    QString error() const { return m_error; }

    Q_INVOKABLE void refresh();
    Q_INVOKABLE void renameFace(const QString &oldName, const QString &newName);
    Q_INVOKABLE void deleteFace(const QString &name);
    Q_INVOKABLE bool hasFace(const QString &name) const;

Q_SIGNALS:
    void errorChanged();

private:
    using Reply = std::function<void(const QDBusMessage &)>;
    void call(const QString &method, const QVariantList &args, Reply done = {});
    void callFaceMethod(const QString &method, const QVariantList &args);
    void setError(const QString &error);

    QString m_user;
    QString m_error;
    FaceModel m_faces;
};
