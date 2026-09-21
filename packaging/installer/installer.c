/* The Windows installer: a welcome, a folder, a progress bar that says what it
 * is doing, shortcuts, an Add/Remove Programs entry and an uninstaller.
 *
 * It is a plain Win32 program built with the same mingw toolchain that
 * cross-compiles the editor, so making a release needs nothing installed that
 * building the program did not already need. Its payload is appended to it by
 * `pack.py`, whose comment describes the format, and expanded by `inflate.c`.
 *
 * Installing over an existing installation is an update, not a second copy:
 * the registry entry it wrote last time says which version is there and in
 * which folder, so it offers that folder rather than the default, says in
 * plain words whether this is an update, a reinstall or a step back to an
 * older version, and deletes what the old version recorded in
 * `installed-files.txt` before writing the new files. A file the person put in
 * that folder themselves is not recorded and so is not touched. There is only
 * ever one entry in Add or remove programs, because it is one registry key
 * written again.
 *
 * It installs for the person running it and asks for no administrator rights:
 * the files go under %LOCALAPPDATA%, the shortcuts under the user's own Start
 * Menu, and the Add/Remove Programs entry under HKEY_CURRENT_USER. Nothing is
 * written to %APPDATA%, and nothing outside the install directory, the two
 * shortcuts and that one registry key is touched.
 *
 * The work happens on a second thread so the progress bar actually moves; the
 * thread posts WM_APP_STEP to the window and the window paints. */

#include <windows.h>
#include <commctrl.h>
#include <shlobj.h>
#include <objbase.h>
#include <stdio.h>
#include <stdlib.h>
#include <wchar.h>
#include "inflate.h"
#include "records.h"

#define PRODUCT        L"PanPDF"
#define EXECUTABLE     L"panpdf.exe"
#define REGISTRY_KEY   L"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PanPDF"
#define PUBLISHER      L"Soukvanhthone Company"
#ifndef PANPDF_VERSION
#define PANPDF_VERSION L"0.0.0"
#endif

#define ID_PATH      101
#define ID_BROWSE    102
#define ID_DESKTOP   103
#define ID_NEXT      104
#define ID_CANCEL    105
#define ID_PROGRESS  106
#define ID_STATUS    107
#define ID_RUN       108
#define ID_HEADING   109
#define ID_BODY      110
#define ID_ROOM      111

#define WM_APP_STEP  (WM_APP + 1)
#define WM_APP_DONE  (WM_APP + 2)

/* ------------------------------------------------------------------ payload */

struct entry {
    wchar_t name[MAX_PATH];
    int method;
    unsigned long compressed, raw, crc;
    const unsigned char *body;
};

static struct entry *entries;
static unsigned long entry_count;
static unsigned long payload_bytes;
static const unsigned char *mapped;

static unsigned long u32(const unsigned char *p)
{
    return (unsigned long)p[0] | ((unsigned long)p[1] << 8)
         | ((unsigned long)p[2] << 16) | ((unsigned long)p[3] << 24);
}

static unsigned long crc32_of(const unsigned char *data, unsigned long length)
{
    static unsigned long table[256];
    static int ready = 0;
    if (!ready) {
        for (unsigned long i = 0; i < 256; i++) {
            unsigned long c = i;
            for (int k = 0; k < 8; k++) c = (c & 1) ? 0xEDB88320UL ^ (c >> 1) : c >> 1;
            table[i] = c;
        }
        ready = 1;
    }
    unsigned long c = 0xFFFFFFFFUL;
    for (unsigned long i = 0; i < length; i++) c = table[(c ^ data[i]) & 0xFF] ^ (c >> 8);
    return c ^ 0xFFFFFFFFUL;
}

/* Map this very file and find the table `pack.py append` wrote onto its end. */
static int open_payload(void)
{
    wchar_t self[MAX_PATH];
    if (!GetModuleFileNameW(NULL, self, MAX_PATH)) return 0;
    HANDLE file = CreateFileW(self, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (file == INVALID_HANDLE_VALUE) return 0;
    LARGE_INTEGER size;
    if (!GetFileSizeEx(file, &size)) { CloseHandle(file); return 0; }
    HANDLE map = CreateFileMappingW(file, NULL, PAGE_READONLY, 0, 0, NULL);
    CloseHandle(file);
    if (!map) return 0;
    mapped = MapViewOfFile(map, FILE_MAP_READ, 0, 0, 0);
    CloseHandle(map);
    if (!mapped) return 0;

    unsigned long long total = (unsigned long long)size.QuadPart;
    if (total < 16 || memcmp(mapped + total - 8, "PANPDFTR", 8) != 0) return 0;
    unsigned long long offset = 0;
    memcpy(&offset, mapped + total - 16, 8);
    if (offset + 12 > total || memcmp(mapped + offset, "PANPDFAR", 8) != 0) return 0;

    const unsigned char *at = mapped + offset + 8;
    entry_count = u32(at); at += 4;
    if (entry_count == 0 || entry_count > 100000) return 0;
    entries = HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, entry_count * sizeof *entries);
    if (!entries) return 0;

    for (unsigned long i = 0; i < entry_count; i++) {
        unsigned long name_len = u32(at); at += 4;
        if (name_len == 0 || name_len >= MAX_PATH) return 0;
        char utf8[MAX_PATH];
        memcpy(utf8, at, name_len); utf8[name_len] = 0; at += name_len;
        if (!MultiByteToWideChar(CP_UTF8, 0, utf8, -1, entries[i].name, MAX_PATH)) return 0;
        for (wchar_t *c = entries[i].name; *c; c++) if (*c == L'/') *c = L'\\';
        /* A name that climbs out of the install folder is refused, not repaired. */
        if (entries[i].name[0] == L'\\' || wcsstr(entries[i].name, L"..")) return 0;
        entries[i].method = at[0];
        entries[i].compressed = u32(at + 1);
        entries[i].raw = u32(at + 5);
        entries[i].crc = u32(at + 9);
        at += 13;
        payload_bytes += entries[i].raw;
    }
    for (unsigned long i = 0; i < entry_count; i++) {
        entries[i].body = at;
        at += entries[i].compressed;
        if ((unsigned long long)(at - mapped) > total) return 0;
    }
    return 1;
}

/* ------------------------------------------------------------- the machinery */

static HWND window, path_box, browse, desktop_box, next_button, cancel_button;
static HWND progress, status, heading, body_text, run_box, room;
static HFONT ui_font, heading_font;
static int page = 0;         /* 0 welcome, 1 working, 2 done */
static int scale = 96;
static volatile int cancelled = 0;
static wchar_t install_dir[MAX_PATH];
static wchar_t failure[512];

/* What is already installed, from the entry this installer wrote last time. */
static int already = 0;          /* 1 if the registry knows an installation */
static int newness = 0;          /* this installer against that one: 1, 0, -1 */
static wchar_t here_version[64];
static wchar_t here_folder[MAX_PATH];

static int dp(int value) { return MulDiv(value, scale, 96); }

/* Four numbers, most significant first. 1 if `a` is newer than `b`, -1 if it
 * is older, 0 if they are the same version. Anything unparseable reads as 0,
 * so "0.2" and "0.2.0" are the same version and neither is an update. */
static int compare_versions(const wchar_t *a, const wchar_t *b)
{
    for (int part = 0; part < 4; part++) {
        wchar_t *after_a = NULL, *after_b = NULL;
        long left = wcstol(a, &after_a, 10);
        long right = wcstol(b, &after_b, 10);
        if (left != right) return left > right ? 1 : -1;
        a = after_a; b = after_b;
        if (*a == L'.') a++;
        if (*b == L'.') b++;
    }
    return 0;
}

static void read_what_is_installed(void)
{
    HKEY key;
    if (RegOpenKeyExW(HKEY_CURRENT_USER, REGISTRY_KEY, 0, KEY_READ, &key) != ERROR_SUCCESS) return;
    DWORD size = sizeof here_version, type = 0;
    if (RegQueryValueExW(key, L"DisplayVersion", NULL, &type, (BYTE *)here_version, &size)
            == ERROR_SUCCESS && type == REG_SZ) {
        size = sizeof here_folder;
        if (RegQueryValueExW(key, L"InstallLocation", NULL, &type, (BYTE *)here_folder, &size)
                == ERROR_SUCCESS && type == REG_SZ && here_folder[0]) {
            already = 1;
            newness = compare_versions(PANPDF_VERSION, here_version);
        }
    }
    RegCloseKey(key);
}

/* Windows will not let a running executable be overwritten, and half-writing
 * over one is the worst thing an updater can do. Opening each of ours for
 * writing, sharing nothing, asks the question without starting anything. */
static int still_open(const wchar_t *folder)
{
    static const wchar_t *const ours[] = { EXECUTABLE, L"panpdf-cli.exe", L"uninstall.exe" };
    for (int i = 0; i < 3; i++) {
        wchar_t path[MAX_PATH];
        swprintf(path, MAX_PATH, L"%ls\\%ls", folder, ours[i]);
        if (GetFileAttributesW(path) == INVALID_FILE_ATTRIBUTES) continue;
        HANDLE file = CreateFileW(path, GENERIC_WRITE, 0, NULL, OPEN_EXISTING, 0, NULL);
        if (file == INVALID_HANDLE_VALUE) {
            if (GetLastError() == ERROR_SHARING_VIOLATION) return 1;
        } else {
            CloseHandle(file);
        }
    }
    return 0;
}

static void megabytes(unsigned long long bytes, wchar_t *into, int room_for)
{
    if (bytes >= 1024ULL * 1024 * 1024)
        swprintf(into, room_for, L"%.1f GB", (double)bytes / (1024.0 * 1024 * 1024));
    else
        swprintf(into, room_for, L"%.0f MB", (double)bytes / (1024.0 * 1024));
}

/* What the choice costs and what the chosen drive has. Windows answers for the
 * nearest existing ancestor of a folder that does not exist yet, so the line
 * is right even while the person is still typing a new path. */
static void say_room(void)
{
    wchar_t chosen[MAX_PATH], needs[32], free_space[32], line[256];
    GetWindowTextW(path_box, chosen, MAX_PATH);
    megabytes(payload_bytes, needs, 32);

    ULARGE_INTEGER available;
    available.QuadPart = 0;
    while (chosen[0]) {
        if (GetDiskFreeSpaceExW(chosen, &available, NULL, NULL)) break;
        available.QuadPart = 0;
        wchar_t *slash = wcsrchr(chosen, L'\\');
        if (!slash) break;
        if (slash == chosen + 2) {
            /* "C:\folder" -- ask the drive itself and then give up. */
            chosen[3] = 0;
            if (!GetDiskFreeSpaceExW(chosen, &available, NULL, NULL)) available.QuadPart = 0;
            break;
        }
        *slash = 0;
    }
    if (available.QuadPart == 0) {
        swprintf(line, 256, L"Needs %ls. That drive did not answer how much room it has.", needs);
    } else {
        megabytes(available.QuadPart, free_space, 32);
        if (available.QuadPart < payload_bytes)
            swprintf(line, 256, L"Needs %ls, and only %ls is free here. Choose somewhere else.",
                     needs, free_space);
        else
            swprintf(line, 256, L"Needs %ls. %ls free.", needs, free_space);
    }
    SetWindowTextW(room, line);
}

static void make_directory(const wchar_t *path)
{
    wchar_t partial[MAX_PATH];
    wcscpy(partial, path);
    for (wchar_t *c = partial + 3; *c; c++) {
        if (*c == L'\\') { *c = 0; CreateDirectoryW(partial, NULL); *c = L'\\'; }
    }
    CreateDirectoryW(partial, NULL);
}

static void parent_of(const wchar_t *path, wchar_t *into)
{
    wcscpy(into, path);
    wchar_t *slash = wcsrchr(into, L'\\');
    if (slash) *slash = 0;
}

static int write_file(const wchar_t *path, const void *data, unsigned long length)
{
    wchar_t folder[MAX_PATH];
    parent_of(path, folder);
    make_directory(folder);
    HANDLE file = CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (file == INVALID_HANDLE_VALUE) return 0;
    DWORD written = 0;
    BOOL ok = length == 0 || WriteFile(file, data, length, &written, NULL);
    CloseHandle(file);
    return ok && written == length;
}

static int make_shortcut(const wchar_t *link, const wchar_t *target,
                         const wchar_t *working, const wchar_t *note)
{
    IShellLinkW *shell = NULL;
    IPersistFile *persist = NULL;
    HRESULT hr = CoCreateInstance(&CLSID_ShellLink, NULL, CLSCTX_INPROC_SERVER,
                                  &IID_IShellLinkW, (void **)&shell);
    if (FAILED(hr)) return 0;
    shell->lpVtbl->SetPath(shell, target);
    shell->lpVtbl->SetWorkingDirectory(shell, working);
    shell->lpVtbl->SetDescription(shell, note);
    shell->lpVtbl->SetIconLocation(shell, target, 0);
    hr = shell->lpVtbl->QueryInterface(shell, &IID_IPersistFile, (void **)&persist);
    if (SUCCEEDED(hr)) {
        wchar_t folder[MAX_PATH];
        parent_of(link, folder);
        make_directory(folder);
        hr = persist->lpVtbl->Save(persist, link, TRUE);
        persist->lpVtbl->Release(persist);
    }
    shell->lpVtbl->Release(shell);
    return SUCCEEDED(hr);
}

static void set_string(HKEY key, const wchar_t *name, const wchar_t *value)
{
    RegSetValueExW(key, name, 0, REG_SZ, (const BYTE *)value,
                   (DWORD)((wcslen(value) + 1) * sizeof(wchar_t)));
}

struct step { int index; const wchar_t *what; int unpacking; };

static void tell_window(int index, const wchar_t *what, int unpacking)
{
    struct step say = { index, what, unpacking };
    SendMessageW(window, WM_APP_STEP, 0, (LPARAM)&say);
}

static void removing(void *context, const wchar_t *what, int done, int total)
{
    (void)done; (void)total;
    tell_window(0, what, 2);
    (void)context;
}

/* Undo the installation recorded in `folder`, if there is one. */
static void remove_recorded(const wchar_t *folder)
{
    wchar_t *lines = records_read(folder);
    if (!lines) return;
    records_delete(folder, lines, removing, NULL);
    records_free(lines);
    records_prune(folder);
}

static DWORD WINAPI install_thread(LPVOID unused)
{
    (void)unused;
    HANDLE heap = GetProcessHeap();
    wchar_t log[32768];
    log[0] = 0;

    CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);

    // An older, newer or identical version already here is undone first, so a
    // file that version shipped and this one does not cannot outlive it. Both
    // the folder being installed into and the folder the registry knows about
    // are undone, which is what makes "update, but somewhere else" clean.
    if (!cancelled) {
        tell_window(0, L"Looking for a version already installed", 0);
        remove_recorded(install_dir);
        if (already && _wcsicmp(here_folder, install_dir) != 0) remove_recorded(here_folder);
    }

    make_directory(install_dir);

    for (unsigned long i = 0; i < entry_count && !cancelled; i++) {
        tell_window((int)i + 1, entries[i].name, 1);

        unsigned char *raw = HeapAlloc(heap, 0, entries[i].raw ? entries[i].raw : 1);
        if (!raw) { swprintf(failure, 512, L"Out of memory unpacking %ls.", entries[i].name); break; }
        if (entries[i].method) {
            if (inflate_raw(entries[i].body, entries[i].compressed, raw, entries[i].raw)) {
                swprintf(failure, 512, L"%ls is damaged. Please download the installer again.", entries[i].name);
                HeapFree(heap, 0, raw);
                break;
            }
        } else {
            memcpy(raw, entries[i].body, entries[i].raw);
        }
        if (crc32_of(raw, entries[i].raw) != entries[i].crc) {
            swprintf(failure, 512, L"%ls did not survive the download intact.", entries[i].name);
            HeapFree(heap, 0, raw);
            break;
        }

        wchar_t target[MAX_PATH];
        swprintf(target, MAX_PATH, L"%ls\\%ls", install_dir, entries[i].name);
        if (!write_file(target, raw, entries[i].raw)) {
            swprintf(failure, 512, L"Could not write\n%ls\n\nIs the folder writable?", target);
            HeapFree(heap, 0, raw);
            break;
        }
        HeapFree(heap, 0, raw);
        if (wcslen(log) + wcslen(entries[i].name) + 4 < 32768) {
            wcscat(log, L"F"); wcscat(log, entries[i].name); wcscat(log, L"\n");
        }
    }

    wchar_t program[MAX_PATH], uninstaller[MAX_PATH];
    swprintf(program, MAX_PATH, L"%ls\\%ls", install_dir, EXECUTABLE);
    swprintf(uninstaller, MAX_PATH, L"%ls\\uninstall.exe", install_dir);

    if (!failure[0] && !cancelled) {
        tell_window((int)entry_count + 1, L"Creating shortcuts", 0);

        wchar_t menu[MAX_PATH], link[MAX_PATH];
        if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_PROGRAMS, NULL, 0, menu))) {
            swprintf(link, MAX_PATH, L"%ls\\%ls.lnk", menu, PRODUCT);
            if (make_shortcut(link, program, install_dir, L"Read and edit PDF documents")) {
                wcscat(log, L"L"); wcscat(log, link); wcscat(log, L"\n");
            }
        }
        if (SendMessageW(desktop_box, BM_GETCHECK, 0, 0) == BST_CHECKED
            && SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_DESKTOPDIRECTORY, NULL, 0, menu))) {
            swprintf(link, MAX_PATH, L"%ls\\%ls.lnk", menu, PRODUCT);
            if (make_shortcut(link, program, install_dir, L"Read and edit PDF documents")) {
                wcscat(log, L"L"); wcscat(log, link); wcscat(log, L"\n");
            }
        }
    }

    if (!failure[0] && !cancelled) {
        tell_window((int)entry_count + 2, L"Registering with Windows", 0);

        wchar_t log_path[MAX_PATH], quoted[MAX_PATH + 4];
        swprintf(log_path, MAX_PATH, L"%ls\\installed-files.txt", install_dir);
        int bytes = WideCharToMultiByte(CP_UTF8, 0, log, -1, NULL, 0, NULL, NULL);
        char *utf8 = HeapAlloc(heap, 0, (size_t)bytes + 1);
        WideCharToMultiByte(CP_UTF8, 0, log, -1, utf8, bytes, NULL, NULL);
        write_file(log_path, utf8, (unsigned long)(bytes - 1));
        HeapFree(heap, 0, utf8);

        HKEY key;
        if (RegCreateKeyExW(HKEY_CURRENT_USER, REGISTRY_KEY, 0, NULL, 0,
                            KEY_WRITE, NULL, &key, NULL) == ERROR_SUCCESS) {
            set_string(key, L"DisplayName", PRODUCT);
            set_string(key, L"DisplayVersion", PANPDF_VERSION);
            set_string(key, L"Publisher", PUBLISHER);
            set_string(key, L"DisplayIcon", program);
            set_string(key, L"InstallLocation", install_dir);
            swprintf(quoted, MAX_PATH + 4, L"\"%ls\"", uninstaller);
            set_string(key, L"UninstallString", quoted);
            swprintf(quoted, MAX_PATH + 4, L"\"%ls\" /quiet", uninstaller);
            set_string(key, L"QuietUninstallString", quoted);
            DWORD one = 1, kilobytes = (DWORD)(payload_bytes / 1024);
            RegSetValueExW(key, L"NoModify", 0, REG_DWORD, (const BYTE *)&one, sizeof one);
            RegSetValueExW(key, L"NoRepair", 0, REG_DWORD, (const BYTE *)&one, sizeof one);
            RegSetValueExW(key, L"EstimatedSize", 0, REG_DWORD, (const BYTE *)&kilobytes, sizeof kilobytes);
            RegCloseKey(key);
        }
    }

    CoUninitialize();
    PostMessageW(window, WM_APP_DONE, 0, 0);
    return 0;
}

/* -------------------------------------------------------------------- pages */

static void show_page(int which)
{
    page = which;
    int welcome = which == 0, working = which == 1, done = which == 2;
    ShowWindow(path_box, welcome ? SW_SHOW : SW_HIDE);
    ShowWindow(browse, welcome ? SW_SHOW : SW_HIDE);
    ShowWindow(desktop_box, welcome ? SW_SHOW : SW_HIDE);
    ShowWindow(room, welcome ? SW_SHOW : SW_HIDE);
    ShowWindow(progress, working || done ? SW_SHOW : SW_HIDE);
    ShowWindow(status, working || done ? SW_SHOW : SW_HIDE);
    ShowWindow(run_box, done ? SW_SHOW : SW_HIDE);
    EnableWindow(next_button, !working);
    EnableWindow(cancel_button, !done);

    if (welcome) {
        wchar_t title[128], words[1024];
        /* Four different things can happen here, and a person is entitled to be
         * told which one before they press the button. */
        if (!already) {
            swprintf(title, 128, L"Install %ls %ls", PRODUCT, PANPDF_VERSION);
            swprintf(words, 1024,
                L"PanPDF reads and edits PDF documents.\r\n\r\n"
                L"It will be installed for you only, so Windows will not ask for an "
                L"administrator. Nothing is written outside the folder you choose "
                L"below, your Start Menu and one entry in Add or remove programs.");
            SetWindowTextW(next_button, L"Install");
        } else if (newness > 0) {
            swprintf(title, 128, L"Update %ls to %ls", PRODUCT, PANPDF_VERSION);
            swprintf(words, 1024,
                L"PanPDF %ls is already installed in the folder below.\r\n\r\n"
                L"It will be updated to %ls. Everything the old version put there is "
                L"removed first, so nothing it used to ship is left behind; anything "
                L"you put in that folder yourself stays where it is. You will still "
                L"have one entry in Add or remove programs, reading %ls.",
                here_version, PANPDF_VERSION, PANPDF_VERSION);
            SetWindowTextW(next_button, L"Update");
        } else if (newness == 0) {
            swprintf(title, 128, L"Reinstall %ls %ls", PRODUCT, PANPDF_VERSION);
            swprintf(words, 1024,
                L"PanPDF %ls is already installed in the folder below, and this "
                L"installer carries the same version.\r\n\r\n"
                L"It will be replaced with a fresh copy: everything the installed "
                L"version put there is removed first, and anything you put in that "
                L"folder yourself stays where it is.",
                here_version);
            SetWindowTextW(next_button, L"Reinstall");
        } else {
            swprintf(title, 128, L"Go back to %ls %ls", PRODUCT, PANPDF_VERSION);
            swprintf(words, 1024,
                L"PanPDF %ls is installed in the folder below, which is newer than "
                L"the %ls this installer carries.\r\n\r\n"
                L"Going back is allowed, and after a bad update it is the right "
                L"answer. %ls will be removed first and %ls put in its place, and "
                L"Add or remove programs will read %ls.",
                here_version, PANPDF_VERSION, here_version, PANPDF_VERSION, PANPDF_VERSION);
            SetWindowTextW(next_button, L"Go back");
        }
        SetWindowTextW(heading, title);
        wcscat(words, L"\r\n\r\nInstall into — change this to anywhere you can write:");
        SetWindowTextW(body_text, words);
        say_room();
    } else if (working) {
        SetWindowTextW(heading, already ? L"Updating" : L"Installing");
        SetWindowTextW(body_text, already
            ? L"One moment. The version already there is being removed, and the new "
              L"files unpacked into the folder you chose."
            : L"One moment. The files are being unpacked into the folder you chose.");
    } else {
        SetWindowTextW(heading, failure[0] ? L"Not installed"
                                           : (already ? PRODUCT L" " PANPDF_VERSION L" is ready"
                                                      : PRODUCT L" is installed"));
        SetWindowTextW(body_text, failure[0] ? failure :
            L"You will find PanPDF in the Start Menu. To remove it later, open "
            L"Settings \u203a Apps \u203a Installed apps, or run uninstall.exe from "
            L"the folder it was installed into.");
        SetWindowTextW(next_button, L"Finish");
        ShowWindow(run_box, failure[0] ? SW_HIDE : SW_SHOW);
    }
    InvalidateRect(window, NULL, TRUE);
}

static void choose_folder(void)
{
    BROWSEINFOW info;
    wchar_t display[MAX_PATH];
    memset(&info, 0, sizeof info);
    info.hwndOwner = window;
    info.pszDisplayName = display;
    info.lpszTitle = L"Choose a folder for PanPDF";
    info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
    LPITEMIDLIST picked = SHBrowseForFolderW(&info);
    if (!picked) return;
    wchar_t chosen[MAX_PATH];
    if (SHGetPathFromIDListW(picked, chosen)) {
        swprintf(install_dir, MAX_PATH, L"%ls\\%ls", chosen, PRODUCT);
        SetWindowTextW(path_box, install_dir);
    }
    CoTaskMemFree(picked);
}

static void place(HWND control, int x, int y, int w, int h)
{
    MoveWindow(control, dp(x), dp(y), dp(w), dp(h), TRUE);
}

static LRESULT CALLBACK proc(HWND hwnd, UINT message, WPARAM wp, LPARAM lp)
{
    switch (message) {
    case WM_APP_STEP: {
        const struct step *tell = (const struct step *)lp;
        wchar_t line[MAX_PATH + 32];
        SendMessageW(progress, PBM_SETPOS, (WPARAM)tell->index, 0);
        if (tell->unpacking == 1) swprintf(line, MAX_PATH + 32, L"Unpacking  %ls", tell->what);
        else if (tell->unpacking == 2) swprintf(line, MAX_PATH + 32, L"Removing  %ls", tell->what);
        else swprintf(line, MAX_PATH + 32, L"%ls", tell->what);
        SetWindowTextW(status, line);
        UpdateWindow(status);
        UpdateWindow(progress);
        return 0;
    }
    case WM_APP_DONE:
        SendMessageW(progress, PBM_SETPOS, (WPARAM)(entry_count + 3), 0);
        SetWindowTextW(status, failure[0] ? L"Stopped." : L"Done.");
        show_page(2);
        return 0;
    case WM_COMMAND:
        if (LOWORD(wp) == ID_PATH && HIWORD(wp) == EN_CHANGE) { say_room(); return 0; }
        switch (LOWORD(wp)) {
        case ID_BROWSE: choose_folder(); return 0;
        case ID_NEXT:
            if (page == 0) {
                GetWindowTextW(path_box, install_dir, MAX_PATH);
                if (!install_dir[0]) return 0;
                if (still_open(install_dir) || (already && still_open(here_folder))) {
                    MessageBoxW(hwnd,
                        L"PanPDF is open at the moment, and Windows will not let a "
                        L"running program be replaced.\n\nPlease close it, then press "
                        L"the button again.",
                        PRODUCT, MB_OK | MB_ICONWARNING);
                    return 0;
                }
                SendMessageW(progress, PBM_SETRANGE32, 0, (LPARAM)(entry_count + 3));
                SendMessageW(progress, PBM_SETPOS, 0, 0);
                show_page(1);
                CloseHandle(CreateThread(NULL, 0, install_thread, NULL, 0, NULL));
            } else if (page == 2) {
                if (!failure[0] && SendMessageW(run_box, BM_GETCHECK, 0, 0) == BST_CHECKED) {
                    wchar_t program[MAX_PATH];
                    swprintf(program, MAX_PATH, L"%ls\\%ls", install_dir, EXECUTABLE);
                    ShellExecuteW(NULL, L"open", program, NULL, install_dir, SW_SHOWNORMAL);
                }
                DestroyWindow(hwnd);
            }
            return 0;
        case ID_CANCEL:
            if (page == 1) cancelled = 1;
            DestroyWindow(hwnd);
            return 0;
        }
        return 0;
    case WM_CTLCOLORSTATIC:
        SetBkMode((HDC)wp, TRANSPARENT);
        return (LRESULT)GetSysColorBrush(COLOR_WINDOW);
    case WM_ERASEBKGND: {
        RECT area;
        GetClientRect(hwnd, &area);
        FillRect((HDC)wp, &area, GetSysColorBrush(COLOR_WINDOW));
        return 1;
    }
    case WM_CLOSE:
        if (page == 1) {
            if (MessageBoxW(hwnd, L"Stop installing PanPDF?", PRODUCT,
                            MB_YESNO | MB_ICONQUESTION) != IDYES) return 0;
            cancelled = 1;
        }
        DestroyWindow(hwnd);
        return 0;
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(hwnd, message, wp, lp);
}

static HWND child(const wchar_t *class_name, const wchar_t *text, DWORD style, int id)
{
    HWND made = CreateWindowExW(0, class_name, text, WS_CHILD | WS_VISIBLE | style,
                                0, 0, 10, 10, window, (HMENU)(INT_PTR)id,
                                GetModuleHandleW(NULL), NULL);
    SendMessageW(made, WM_SETFONT, (WPARAM)ui_font, TRUE);
    return made;
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, PWSTR command, int show)
{
    (void)previous; (void)command; (void)show;
    INITCOMMONCONTROLSEX controls = { sizeof controls, ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES };
    InitCommonControlsEx(&controls);

    if (!open_payload()) {
        MessageBoxW(NULL, L"This installer is damaged: its payload is missing or "
                          L"does not check out. Please download it again.",
                    PRODUCT, MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST);
        return 1;
    }

    /* The folder already in use beats the default: a person who once chose
     * somewhere else chose it for a reason, and an update should not move. */
    read_what_is_installed();
    if (already) {
        wcscpy(install_dir, here_folder);
    } else {
        wchar_t local[MAX_PATH];
        if (SUCCEEDED(SHGetFolderPathW(NULL, CSIDL_LOCAL_APPDATA, NULL, 0, local)))
            swprintf(install_dir, MAX_PATH, L"%ls\\Programs\\%ls", local, PRODUCT);
        else
            wcscpy(install_dir, L"C:\\PanPDF");
    }

    WNDCLASSEXW class_info;
    memset(&class_info, 0, sizeof class_info);
    class_info.cbSize = sizeof class_info;
    class_info.lpfnWndProc = proc;
    class_info.hInstance = instance;
    class_info.hIcon = LoadIconW(instance, MAKEINTRESOURCEW(1));
    class_info.hIconSm = class_info.hIcon;
    class_info.hCursor = LoadCursorW(NULL, IDC_ARROW);
    class_info.hbrBackground = GetSysColorBrush(COLOR_WINDOW);
    class_info.lpszClassName = L"PanPDFInstaller";
    RegisterClassExW(&class_info);

    HDC screen = GetDC(NULL);
    scale = GetDeviceCaps(screen, LOGPIXELSX);
    ReleaseDC(NULL, screen);

    ui_font = CreateFontW(-dp(12), 0, 0, 0, FW_NORMAL, 0, 0, 0, DEFAULT_CHARSET,
                          OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                          DEFAULT_PITCH | FF_DONTCARE, L"Segoe UI");
    heading_font = CreateFontW(-dp(19), 0, 0, 0, FW_SEMIBOLD, 0, 0, 0, DEFAULT_CHARSET,
                               OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                               DEFAULT_PITCH | FF_DONTCARE, L"Segoe UI");

    RECT wanted = { 0, 0, dp(540), dp(380) };
    AdjustWindowRect(&wanted, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, FALSE);
    window = CreateWindowExW(0, L"PanPDFInstaller", PRODUCT L" " PANPDF_VERSION L" Setup",
                             WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                             CW_USEDEFAULT, CW_USEDEFAULT,
                             wanted.right - wanted.left, wanted.bottom - wanted.top,
                             NULL, NULL, instance, NULL);
    if (!window) return 1;

    heading = child(L"STATIC", L"", 0, ID_HEADING);
    SendMessageW(heading, WM_SETFONT, (WPARAM)heading_font, TRUE);
    body_text = child(L"STATIC", L"", 0, ID_BODY);
    path_box = child(L"EDIT", install_dir, WS_BORDER | ES_AUTOHSCROLL, ID_PATH);
    browse = child(L"BUTTON", L"Browse\u2026", BS_PUSHBUTTON | WS_TABSTOP, ID_BROWSE);
    room = child(L"STATIC", L"", SS_ENDELLIPSIS, ID_ROOM);
    desktop_box = child(L"BUTTON", L"Also put a shortcut on the desktop",
                        BS_AUTOCHECKBOX | WS_TABSTOP, ID_DESKTOP);
    progress = child(PROGRESS_CLASSW, L"", PBS_SMOOTH, ID_PROGRESS);
    status = child(L"STATIC", L"", SS_PATHELLIPSIS, ID_STATUS);
    run_box = child(L"BUTTON", L"Open PanPDF now", BS_AUTOCHECKBOX | WS_TABSTOP, ID_RUN);
    next_button = child(L"BUTTON", L"Install", BS_DEFPUSHBUTTON | WS_TABSTOP, ID_NEXT);
    cancel_button = child(L"BUTTON", L"Cancel", BS_PUSHBUTTON | WS_TABSTOP, ID_CANCEL);

    place(heading, 28, 26, 480, 30);
    place(body_text, 28, 66, 484, 130);
    place(path_box, 28, 200, 380, 26);
    place(browse, 416, 199, 96, 28);
    place(room, 28, 232, 484, 20);
    place(desktop_box, 28, 258, 440, 24);
    place(progress, 28, 200, 484, 22);
    place(status, 28, 232, 484, 22);
    place(run_box, 28, 258, 440, 24);
    place(next_button, 300, 322, 106, 32);
    place(cancel_button, 414, 322, 98, 32);
    SendMessageW(desktop_box, BM_SETCHECK, BST_CHECKED, 0);
    SendMessageW(run_box, BM_SETCHECK, BST_CHECKED, 0);

    show_page(0);
    ShowWindow(window, SW_SHOWNORMAL);
    SetForegroundWindow(window);
    UpdateWindow(window);

    MSG message;
    while (GetMessageW(&message, NULL, 0, 0) > 0) {
        if (!IsDialogMessageW(window, &message)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    return 0;
}
