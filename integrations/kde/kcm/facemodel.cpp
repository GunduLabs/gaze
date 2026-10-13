// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#include "facemodel.h"

int FaceModel::rowCount(const QModelIndex &parent) const
{
    return parent.isValid() ? 0 : m_faces.size();
}

QVariant FaceModel::data(const QModelIndex &index, int role) const
{
    if (!checkIndex(index, CheckIndexOption::IndexIsValid | CheckIndexOption::ParentIsInvalid)) {
        return {};
    }
    const Face &face = m_faces.at(index.row());
    switch (role) {
    case NameRole:
        return face.name;
    case SamplesRole:
        return face.samples;
    case RgbRole:
        return face.rgb;
    case IrRole:
        return face.ir;
    }
    return {};
}

QHash<int, QByteArray> FaceModel::roleNames() const
{
    return {{NameRole, "name"}, {SamplesRole, "samples"}, {RgbRole, "rgb"}, {IrRole, "ir"}};
}

void FaceModel::setFaces(QList<Face> faces)
{
    beginResetModel();
    m_faces = std::move(faces);
    endResetModel();
}

bool FaceModel::contains(const QString &name) const
{
    return std::any_of(m_faces.cbegin(), m_faces.cend(), [&](const Face &face) {
        return face.name == name;
    });
}
