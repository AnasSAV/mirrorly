# Files and clipboard

Everything here goes through the iPhone's own clipboard (iOS's pasteboard
service), so it works with any app, in both directions, while Mirrorly is
connected.

## iPhone → Windows: copy, then drag

1. On the iPhone, **copy** what you want:
   - **Photos:** open a photo › **Share** › **Copy Photo**. For several, tap
     **Select**, pick them, then **Share › Copy Photos**.
   - **Files:** touch and hold a file in Files › **Copy**.
2. Mirrorly fetches it at once, at full quality, and says
   *"Photo copied · drag it from the bar"*. It's also on the Windows
   clipboard: **Ctrl+V** into Explorer, a chat or an email works too, and a
   photo pastes as a picture into Paint, Word and the like.
3. Point at the window and **drag the blue chip** on the bar onto the
   desktop, into a folder, or into any app that takes files.

Files keep their names (`Invoice.pdf` stays `Invoice.pdf`); photos are named
by date and time. Fetched files stay in `%LOCALAPPDATA%\Mirrorly\clipboard`
until you copy something else.

## Windows → iPhone: drop on the window

Drag files from Explorer and drop them on the mirrored screen.

| You drop | What happens on the iPhone |
|---|---|
| **Photos and videos** | Pasted into the app you have open (a chat, a note, an email), as if you pressed ⌘V there. Open the place you want them first. |
| **Documents** (PDF, Word, text, …) | Saved with their names in **Files › On My iPhone › *app* › Mirrorly**, and put on the clipboard too, so you can paste them into a chat. |

iOS doesn't let other programs write to the top level of *On My iPhone*, so
documents go into a **Mirrorly** folder inside an app that shares files with
the PC (VLC or Chrome, for example). In Files, touch and hold that folder and
choose **Favorite** to find it in the sidebar.

Up to 200 MB per drop. Folders aren't sent; drop the files in them.

## One clipboard

| You copy | On | Then |
|---|---|---|
| Text or a picture | Windows | Paste on the iPhone: **Ctrl+V** in Mirrorly, or touch and hold › **Paste** |
| Files (in Explorer) | Windows | **Ctrl+V** in Mirrorly: photos are pasted, documents saved by name |
| Text | iPhone | **Ctrl+V** anywhere on the PC |
| Photos or files | iPhone | **Ctrl+V** on the PC (and the chip on the bar) |

The newest copy wins: copy on the iPhone after the PC, and Ctrl+V in
Mirrorly pastes the iPhone's copy.

**Privacy.** Anything a password manager marks as private on the PC is never
sent to the iPhone, and text that comes from the iPhone is kept out of
Windows' clipboard history and cloud clipboard. On the iPhone the paste
suggestion reads *"Paste from dtpasteboardd"*: that is the iOS service
Mirrorly hands the copy to, and iOS doesn't let it be renamed.
