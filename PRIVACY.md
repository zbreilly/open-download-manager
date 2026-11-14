# Privacy

Open Download Manager and its browser extension don't collect anything about you. Neither one sends data to the project, to its authors or to any third party. There is no account, no analytics, no crash reporting and no update check.

Last updated: 14 November 2026.

## The browser extension

The extension talks to one thing: the Open Download Manager app on your own computer, at `127.0.0.1`. It contacts no other host.

When you start a download that the extension hands over, or use its menu entries, popup or Download button, it sends the app:

- the address of the file or stream
- the cookies your browser holds for that address
- the address of the page you were on
- your browser's user agent
- the file's name, size and type when they are known

The app needs these to fetch the file the way your browser would, including from sites you are logged in to. Everything is encrypted with a key that you copy from the app into the extension, so another program on your computer can't read it or pose as the app.

The extension keeps a list of the video and audio the pages in your open tabs have loaded, so it can show them in its popup. That list stays in the browser and is not sent anywhere unless you press Download on an item.

The extension stores two things in your browser: the pairing key and the port the app listens on.

Downloads in private or incognito windows are not taken over automatically.

## The app

The app connects to the servers you download from, through a proxy if you or your system have set one. It makes no other connections.

It keeps your download list, queues and settings in a database on your computer. Cookies, passwords and custom headers saved for a download are encrypted with a key held by your system's keystore, and are removed from the download once it has finished.

## Questions

Open an issue at https://github.com/zbreilly/open-download-manager/issues.
