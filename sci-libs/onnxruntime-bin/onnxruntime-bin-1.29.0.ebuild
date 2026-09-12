# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

MY_P="onnxruntime-linux-x64-${PV}"

DESCRIPTION="Cross-platform machine-learning inferencing and training accelerator"
HOMEPAGE="https://onnxruntime.ai/"
SRC_URI="https://github.com/microsoft/onnxruntime/releases/download/v${PV}/${MY_P}.tgz"
S="${WORKDIR}/${MY_P}"

LICENSE="MIT"
SLOT="0/1"
KEYWORDS="~amd64"

DOCS=(
	LICENSE
	Privacy.md
	ThirdPartyNotices.txt
	VERSION_NUMBER
)

QA_PREBUILT="usr/lib*/lib*.so*"

src_prepare() {
	default
	sed -i \
		-e 's|^prefix=/usr/local$|prefix=/usr|' \
		-e "s|^libdir=.*|libdir=\${prefix}/$(get_libdir)|" \
		lib/pkgconfig/libonnxruntime.pc || die
}

src_install() {
	insinto /usr/include/onnxruntime
	doins -r include/.

	dolib.so lib/lib*.so*

	insinto "/usr/$(get_libdir)/pkgconfig"
	doins lib/pkgconfig/libonnxruntime.pc

	insinto "/usr/$(get_libdir)/cmake/onnxruntime"
	doins lib/cmake/onnxruntime/*

	einstalldocs
}
