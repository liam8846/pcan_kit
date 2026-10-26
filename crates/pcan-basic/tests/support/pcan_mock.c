/* Linux PCAN-Basic 測試替身：用 eventfd 驗證 epoll 與通道清理順序。 */
#include <dirent.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/eventfd.h>
#include <unistd.h>

static int driver_fd = -1;
static int violations;
static int closes;
static int fail_filter;
static int reads_after_close;

static int event_id(int fd) {
    char path[80], line[256];
    int id = -1;
    snprintf(path, sizeof(path), "/proc/self/fdinfo/%d", fd);
    FILE *info = fopen(path, "r");
    if (!info) return -1;
    while (fgets(line, sizeof(line), info)) {
        if (sscanf(line, "eventfd-id: %d", &id) == 1) break;
    }
    fclose(info);
    return id;
}

/* 連同複製的 fd 一起查，避免原始 fd 與 reactor fd 不同而漏測。 */
static int registrations(void) {
    int id = event_id(driver_fd), count = 0;
    DIR *directory = opendir("/proc/self/fdinfo");
    if (!directory || id < 0) return -1;
    struct dirent *entry;
    while ((entry = readdir(directory))) {
        char path[512], line[512];
        snprintf(path, sizeof(path), "/proc/self/fdinfo/%s", entry->d_name);
        FILE *info = fopen(path, "r");
        if (!info) continue;
        while (fgets(line, sizeof(line), info)) {
            int target;
            if (sscanf(line, "tfd: %d", &target) == 1 && event_id(target) == id) count++;
        }
        fclose(info);
    }
    closedir(directory);
    return count;
}

uint32_t CAN_Initialize(uint16_t h, uint16_t b, uint8_t t, uint32_t p, uint16_t i) {
    driver_fd = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);
    return driver_fd < 0 ? 0x04000000 : 0;
}
uint32_t CAN_Uninitialize(uint16_t h) {
    if (registrations() != 0) violations++;
    if (close(driver_fd) != 0) violations++;
    driver_fd = -1;
    closes++;
    return 0;
}
uint32_t CAN_GetValue(uint16_t h, uint8_t parameter, void *buffer, uint32_t len) {
    int value;
    switch (parameter) {
        case 3: value = driver_fd; break;
        case 0xe0: value = violations; break;
        case 0xe2: value = closes; break;
        case 0xe3: value = reads_after_close; break;
        case 0xe5: value = registrations(); break;
        default: return 0x400;
    }
    if (len != sizeof(value)) return 0x800;
    memcpy(buffer, &value, sizeof(value));
    return 0;
}
uint32_t CAN_SetValue(uint16_t h, uint8_t parameter, void *buffer, uint32_t len) {
    if (parameter == 0xe1) memcpy(&fail_filter, buffer, sizeof(fail_filter));
    return parameter == 4 && fail_filter ? 0x04000000 : 0;
}
uint32_t CAN_Read(uint16_t h, void *message, void *timestamp) {
    if (driver_fd < 0) reads_after_close++;
    return 0x20;
}
uint32_t CAN_Write(uint16_t h, void *message) { return 0; }
uint32_t CAN_Reset(uint16_t h) { return 0; }
uint32_t CAN_GetStatus(uint16_t h) { return 0; }
uint32_t CAN_FilterMessages(uint16_t h, uint32_t from, uint32_t to, uint8_t mode) { return 0; }
uint32_t CAN_GetErrorText(uint32_t error, uint16_t language, char *buffer) {
    if (error == UINT32_MAX) {
        memset(buffer, 'X', 256);
        return 0;
    }
    strcpy(buffer, "mock error");
    return 0;
}
