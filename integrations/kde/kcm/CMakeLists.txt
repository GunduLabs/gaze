# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

cmake_minimum_required(VERSION 3.20)
project(kcm_gaze LANGUAGES CXX)

find_package(ECM 6.0 REQUIRED NO_MODULE)
set(CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

include(KDEInstallDirs)
include(KDECMakeSettings)
include(KDECompilerSettings NO_POLICY_SCOPE)

find_package(Qt6 REQUIRED COMPONENTS Core DBus Quick)
find_package(KF6 REQUIRED COMPONENTS CoreAddons I18n KCMUtils Kirigami)

kcmutils_add_qml_kcm(kcm_gaze SOURCES dbuscall.cpp gazekcm.cpp facemodel.cpp gazesettings.cpp)
target_link_libraries(kcm_gaze PRIVATE Qt6::Core Qt6::DBus Qt6::Quick KF6::CoreAddons KF6::I18n KF6::KCMUtilsQuick)
install(FILES com.gundulabs.Gaze.Settings.desktop DESTINATION ${KDE_INSTALL_APPDIR})
install(FILES ../../../packaging/gui/com.gundulabs.Gaze.svg DESTINATION ${KDE_INSTALL_ICONDIR}/hicolor/scalable/apps RENAME com.gundulabs.Gaze.Settings.svg)
