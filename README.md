<h1 align="center">
    <picture>
      <source height="125" media="(prefers-color-scheme: dark)" srcset="assets/broma.svg">
      <img height="125" alt="Broma" src="assets/broma.svg">
    </picture>
</h1>
<h2 align="center">Broma</h2>
<p align="center">A Rust-based lightweight GUI Soundboard application, designed to be fast and efficient.</p>

## How does it work?
You can play short(or long, if you're one of those people) customized audio clips 
through a Virtual Microphone(eg: VB Cables for Windows) 
that is then audible to other people when set as microphone in other applications.

# Installation

### 🪟 Windows

The app is available for both [x86_64 and Windows ARM](https://github.com/s1codes/broma/releases/latest)

but for it to work properly, You need an additional software which is a Virtual Microphone Device

I suggest you use [VB Cable](https://vb-audio.com/Cable/index.htm) and then set it as the Output in the Broma App

Then use that Virtual Cable Driver as the primary Microphone in the apps you want the soundboard to work at.. or just set it as system-default if you're crazy like that

## 🐧 Linux

Broma is available to download as [Debian and RPM Package (For Fedora)](https://github.com/s1codes/broma/releases/latest)
and for other distros, the app is also available as an [AppImage](https://github.com/s1codes/broma/releases/latest)

All packages also have their respective [AArch64/ARM64](https://github.com/s1codes/broma/releases/latest) package available to download

Usually, since most distros are running PipeWire.. You do not need any additional software, as Broma will create one Virtual Microphone using native PipeWire APIs

Just make sure you do not forget to select that as the Microphone in the selected apps you want the soundboard to work at... or just set it as system-default if you feel that aura inside you 😉


## 🍎 macOS

~~f**k off i ain't that rich bro~~

macOS is currently unsupported and their packages are also unavailable to download. 
Since I do not have the appropriate device(s) to test this app in. also i have no idea what virtual device you would use there or what apis are used to control audio, ngl. Educate me if you know! 

## LICENSE
This project is licensed under the [The 3-Clause BSD License](LICENSE)
