/* Reading and undoing an `installed-files.txt`. See records.h for the format.
 *
 * This is shared because the installer and the uninstaller must agree, to the
 * file, on what "what the old version put there" means. When they disagree an
 * update either leaves junk behind or deletes something it did not install,
 * and both of those are found by the person, not by us. */

#include "records.h"

wchar_t *records_read(const wchar_t *folder)
{
    wchar_t record[MAX_PATH];
    swprintf(record, MAX_PATH, L"%ls\\installed-files.txt", folder);

    HANDLE file = CreateFileW(record, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (file == INVALID_HANDLE_VALUE) return NULL;
    DWORD size = GetFileSize(file, NULL), got = 0;
    if (size == INVALID_FILE_SIZE || size > 4u * 1024 * 1024) { CloseHandle(file); return NULL; }

    char *utf8 = HeapAlloc(GetProcessHeap(), 0, (size_t)size + 1);
    if (!utf8) { CloseHandle(file); return NULL; }
    BOOL ok = ReadFile(file, utf8, size, &got, NULL);
    CloseHandle(file);
    if (!ok) { HeapFree(GetProcessHeap(), 0, utf8); return NULL; }
    utf8[got] = 0;

    int wide = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, NULL, 0);
    wchar_t *lines = wide > 0 ? HeapAlloc(GetProcessHeap(), 0, (size_t)wide * sizeof(wchar_t)) : NULL;
    if (lines) MultiByteToWideChar(CP_UTF8, 0, utf8, -1, lines, wide);
    HeapFree(GetProcessHeap(), 0, utf8);
    return lines;
}

void records_free(wchar_t *lines)
{
    if (lines) HeapFree(GetProcessHeap(), 0, lines);
}

int records_count(const wchar_t *lines)
{
    int total = 0;
    if (!lines) return 0;
    for (const wchar_t *c = lines; *c; c++)
        if (*c == L'\n') total++;
    return total;
}

int records_delete(const wchar_t *folder, wchar_t *lines, records_saying saying, void *context)
{
    if (!lines) return 0;
    int total = records_count(lines), done = 0;
    wchar_t *line = lines;

    while (*line) {
        wchar_t *end = line;
        while (*end && *end != L'\n' && *end != L'\r') end++;
        wchar_t saved = *end;
        *end = 0;
        if (line[0] == L'F' || line[0] == L'L') {
            wchar_t target[MAX_PATH];
            if (line[0] == L'F') swprintf(target, MAX_PATH, L"%ls\\%ls", folder, line + 1);
            else wcsncpy(target, line + 1, MAX_PATH - 1), target[MAX_PATH - 1] = 0;
            done++;
            if (saying) saying(context, target, done, total);
            SetFileAttributesW(target, FILE_ATTRIBUTE_NORMAL);
            DeleteFileW(target);
        }
        *end = saved;
        line = end;
        while (*line == L'\n' || *line == L'\r') line++;
    }

    wchar_t record[MAX_PATH];
    swprintf(record, MAX_PATH, L"%ls\\installed-files.txt", folder);
    DeleteFileW(record);
    return done;
}

void records_prune(const wchar_t *root)
{
    wchar_t pattern[MAX_PATH];
    WIN32_FIND_DATAW found;
    swprintf(pattern, MAX_PATH, L"%ls\\*", root);
    HANDLE search = FindFirstFileW(pattern, &found);
    if (search == INVALID_HANDLE_VALUE) return;
    do {
        if (!(found.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) continue;
        if (!wcscmp(found.cFileName, L".") || !wcscmp(found.cFileName, L"..")) continue;
        wchar_t child[MAX_PATH];
        swprintf(child, MAX_PATH, L"%ls\\%ls", root, found.cFileName);
        records_prune(child);
        RemoveDirectoryW(child);
    } while (FindNextFileW(search, &found));
    FindClose(search);
}
