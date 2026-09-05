#include <sys/types.h>
#include <sys/mount.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <aio.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) return 90;
    alarm(30);
    if (!strcmp(argv[1], "unmount")) {
        errno = 0; int result = unmount(argv[2], 0); int error = errno;
        printf("unmount=%d errno=%d\n", result, error);
        return result == 0 ? 0 : error == EBUSY ? 16 : 91;
    }
    if (!strcmp(argv[1], "flags")) {
        struct statfs fs;
        if (statfs(argv[2], &fs)) return 92;
        printf("filesystem=%s readonly=%d device=%s\n", fs.f_fstypename,
               !!(fs.f_flags & MNT_RDONLY), fs.f_mntfromname);
        return strcmp(fs.f_fstypename, "apfs") ? 93 : 0;
    }
    if (!strcmp(argv[1], "readonly")) {
        errno = 0; int fd = open(argv[2], O_WRONLY); int error = errno;
        printf("write_open=%d errno=%d\n", fd, error);
        if (fd >= 0) close(fd);
        return fd < 0 && error == EROFS ? 0 : 94;
    }
    int fd = -1;
    void *mapping = MAP_FAILED;
    if (!strcmp(argv[1], "cwd")) {
        if (chdir(argv[2])) return 95;
    } else {
        fd = open(argv[2], !strcmp(argv[1], "read") ? O_RDONLY : O_RDWR);
        if (fd < 0) return 96;
        if (!strcmp(argv[1], "mmap")) {
            mapping = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
            if (mapping == MAP_FAILED) return 97;
            ((volatile char *)mapping)[0] = 'M';
            if (close(fd)) return 98;
            fd = -1;
        } else if (!strcmp(argv[1], "write")) {
            if (pwrite(fd, "W", 1, 0) != 1) return 99;
        } else if (!strcmp(argv[1], "aio")) {
            struct aiocb op = {0}; char payload = 'A';
            op.aio_fildes = fd; op.aio_buf = &payload; op.aio_nbytes = 1;
            op.aio_sigevent.sigev_notify = SIGEV_NONE;
            if (aio_write(&op)) return 100;
            while (aio_error(&op) == EINPROGRESS) usleep(1000);
            if (aio_error(&op) || aio_return(&op) != 1) return 101;
        } else if (strcmp(argv[1], "read")) return 102;
    }
    puts("READY"); fflush(stdout);
    for (;;) pause();
}
