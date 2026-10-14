// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#pragma once

#include <KQuickConfigModule>
#include <QDBusMessage>
#include <QTimer>
#include <functional>

#include "dbuscall.h"
#include "facemodel.h"
#include "gazesettings.h"

class GazeKcm : public KQuickConfigModule
{
    Q_OBJECT
    Q_PROPERTY(FaceModel *faces READ faces CONSTANT)
    Q_PROPERTY(GazeSettings *settings READ settings CONSTANT)
    Q_PROPERTY(QString user READ user CONSTANT)
    Q_PROPERTY(QString error READ error NOTIFY errorChanged)
    // "idle", "running", "done" or "failed"
    Q_PROPERTY(QString enrollState READ enrollState NOTIFY enrollChanged)
    Q_PROPERTY(QString prompt READ prompt NOTIFY enrollChanged)
    Q_PROPERTY(QString faceHint READ faceHint NOTIFY enrollChanged)
    Q_PROPERTY(uint progress READ progress NOTIFY enrollChanged)
    Q_PROPERTY(uint steps READ steps NOTIFY enrollChanged)
    Q_PROPERTY(QString preview READ preview NOTIFY previewChanged)
    // "idle", "running", "success", "failed" or "timeout"
    Q_PROPERTY(QString testState READ testState NOTIFY testChanged)

public:
    GazeKcm(QObject *parent, const KPluginMetaData &data);
    ~GazeKcm() override;

    FaceModel *faces() { return &m_faces; }
    GazeSettings *settings() { return &m_settings; }
    QString user() const { return m_user; }
    QString error() const { return m_error; }
    QString enrollState() const { return m_enrollState; }
    QString prompt() const { return m_prompt; }
    QString faceHint() const { return m_faceHint; }
    uint progress() const { return m_progress; }
    uint steps() const { return m_steps; }
    QString preview() const { return m_preview; }
    QString testState() const { return m_testState; }

    Q_INVOKABLE void refresh();
    Q_INVOKABLE void renameFace(const QString &oldName, const QString &newName);
    Q_INVOKABLE void deleteFace(const QString &name);
    Q_INVOKABLE bool hasFace(const QString &name) const;
    // V4L2 capture nodes as { text, value } pairs for the camera pickers.
    Q_INVOKABLE QVariantList cameraSources() const;
    Q_INVOKABLE QString suggestedName() const;
    Q_INVOKABLE void startEnroll(const QString &name);
    Q_INVOKABLE void stopEnroll();
    Q_INVOKABLE void testFace();

Q_SIGNALS:
    void errorChanged();
    void enrollChanged();
    void previewChanged();
    void testChanged();

private Q_SLOTS:
    void onEnrollStatus(const QDBusMessage &message);
    void onFaceStatus(const QDBusMessage &message);
    void onPreviewFrame(const QDBusMessage &message);
    void onVerifyStatus(const QDBusMessage &message);

private:
    void call(const QString &method, const QVariantList &args, GazeBus::Reply done = {});
    void callFaceMethod(const QString &method, const QVariantList &args);
    void finishEnroll(const QString &state, const QString &prompt);
    void finishTest(const QString &state);
    void release();
    bool busy() const;
    // Claims the device for the user, runs one method, and reports the first error.
    void claimAnd(const QString &method, const QVariantList &args, std::function<void(const QString &)> failed, std::function<void()> started = {});
    void setError(const QString &error);

    QString m_user;
    QString m_error;
    FaceModel m_faces;
    GazeSettings m_settings;
    QString m_enrollState = QStringLiteral("idle");
    QString m_prompt;
    QString m_faceHint;
    uint m_progress = 0;
    uint m_steps = 0;
    QString m_preview;
    QString m_testState = QStringLiteral("idle");
    QTimer m_testTimeout;
    QTimer m_testReset;
};
