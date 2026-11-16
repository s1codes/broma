%{!?broma_version:%global broma_version 0.1.0}

Name:           broma
Version:        %{broma_version}
Release:        1%{?dist}
Summary:        Low-overhead soundboard for gamers
License:        BSD-3-Clause
Source0:        %{name}-%{version}.tar.gz
Requires:       alsa-lib

%description
Guilt free and efficient Soundboard

%prep
%setup -q -n broma-%{version}

%build
:

%install
install -Dpm0755 broma %{buildroot}%{_bindir}/broma
install -Dpm0644 io.github.s1codes.broma.desktop \
  %{buildroot}%{_datadir}/applications/io.github.s1codes.broma.desktop
install -Dpm0644 broma.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/io.github.s1codes.broma.svg
install -Dpm0644 io.github.s1codes.broma.metainfo.xml \
  %{buildroot}%{_datadir}/metainfo/io.github.s1codes.broma.metainfo.xml

%files
%license LICENSE
%{_bindir}/broma
%{_datadir}/applications/io.github.s1codes.broma.desktop
%{_datadir}/icons/hicolor/scalable/apps/io.github.s1codes.broma.svg
%{_datadir}/metainfo/io.github.s1codes.broma.metainfo.xml
