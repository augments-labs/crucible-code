#define _DARWIN_C_SOURCE 1
#include <errno.h>
#include <grp.h>
#include <mach/mach.h>
#include <mach/exception_types.h>
#include <pwd.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

typedef struct { mach_msg_header_t head; uint64_t payload; } Message;
typedef union { Message message; unsigned char bytes[512]; } Buffer;
static double now(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) _exit(77);
    return t.tv_sec + t.tv_nsec / 1e9;
}
static void checked(kern_return_t value, const char *label) {
    if (value) { fprintf(stderr, "API-FAILED %s result=%d\n", label, value); exit(77); }
}
static Message request(mach_port_t destination, mach_port_t reply) {
    Message m = {0};
    m.head.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, reply ? MACH_MSG_TYPE_MAKE_SEND_ONCE : 0);
    m.head.msgh_size = sizeof m;
    m.head.msgh_remote_port = destination;
    m.head.msgh_local_port = reply;
    m.head.msgh_id = 901;
    m.payload = UINT64_C(0x71624354abcdef19);
    return m;
}
static int reap(pid_t child, double end, int *status) {
    while (now() < end) {
        pid_t result = waitpid(child, status, WNOHANG);
        if (result == child) return 1;
        if (result < 0 && errno != EINTR) return 0;
        struct timespec pause = {0, 10000000}; nanosleep(&pause, NULL);
    }
    return 0;
}
int main(int argc, char **argv) {
    if (argc != 3) return 77;
    setvbuf(stdout, NULL, _IONBF, 0);
    signal(SIGCHLD, SIG_DFL);
    int exception = !strcmp(argv[1], "exception");
    int initialized = !strcmp(argv[2], "lookup");
    if (initialized) { (void)getpwuid(getuid()); (void)getgrgid(getgid()); }
    printf("CASE kind=%s initialization=%s\n", argv[1], argv[2]);
    mach_port_t service = MACH_PORT_NULL;
    if (exception) {
        mach_port_options_t options = {0}; options.flags = MPO_EXCEPTION_PORT;
        checked(mach_port_construct(mach_task_self(), &options, 0, &service), "construct exception receive");
    } else checked(mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &service), "allocate receive");
    checked(mach_port_insert_right(mach_task_self(), service, service, MACH_MSG_TYPE_MAKE_SEND), "insert send");
    Message sent = request(service, MACH_PORT_NULL);
    checked(mach_msg(&sent.head, MACH_SEND_MSG | MACH_SEND_TIMEOUT, sizeof sent, 0, MACH_PORT_NULL, 500, MACH_PORT_NULL), "calibration send");
    Buffer calibration = {0};
    checked(mach_msg(&calibration.message.head, MACH_RCV_MSG | MACH_RCV_TIMEOUT, 0, sizeof calibration, service, 500, MACH_PORT_NULL), "calibration receive");
    if (calibration.message.head.msgh_size != sizeof(Message) || calibration.message.head.msgh_id != 901 || calibration.message.payload != sent.payload) return 77;
    puts("CALIBRATION exact_payload=1");
    mach_port_array_t saved = NULL; mach_msg_type_number_t count = 0;
    checked(mach_ports_lookup(mach_task_self(), &saved, &count), "save slots");
    if (count > 16) return 77;
    checked(mach_ports_register(mach_task_self(), &service, 1), "register transport");
    pid_t child = fork();
    if (child < 0) return 77;
    if (!child) {
        alarm(3);
        mach_port_array_t inherited = NULL; mach_msg_type_number_t size = 0;
        kern_return_t lookup = mach_ports_lookup(mach_task_self(), &inherited, &size);
        if (lookup || !size || size > 16 || !MACH_PORT_VALID(inherited[0])) _exit(77);
        mach_port_t destination = inherited[0];
        kern_return_t setting = exception ? task_set_exception_ports(mach_task_self(), EXC_MASK_BREAKPOINT, destination, EXCEPTION_DEFAULT, THREAD_STATE_NONE) : KERN_SUCCESS;
        mach_port_t reply = MACH_PORT_NULL;
        if (mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &reply)) _exit(77);
        Buffer b = {0}; b.message = request(destination, reply);
        mach_msg_return_t result = mach_msg(&b.message.head, MACH_SEND_MSG | MACH_SEND_TIMEOUT | MACH_RCV_MSG | MACH_RCV_TIMEOUT, sizeof(Message), sizeof b, reply, 500, MACH_PORT_NULL);
        int ack = !result && b.message.head.msgh_size == sizeof(Message) && b.message.head.msgh_id == 902 && b.message.payload == (sent.payload ^ UINT64_C(0xffff));
        printf("CHILD lookup=%d slots=%u exception_set=%d receive=%d size=%u id=%d ack=%d\n", lookup, size, setting, result, b.message.head.msgh_size, b.message.head.msgh_id, ack);
        _exit(ack && !setting ? 0 : 77);
    }
    // Parent keeps its original receive right and restores only the slot references.
    kern_return_t restored = mach_ports_register(mach_task_self(), saved, count);
    int observed = 0, done = 0, status = 0; double end = now() + 5;
    while (now() < end) {
        Buffer b = {0};
        mach_msg_return_t result = mach_msg(&b.message.head, MACH_RCV_MSG | MACH_RCV_TIMEOUT, 0, sizeof b, service, 10, MACH_PORT_NULL);
        if (result == MACH_MSG_SUCCESS) {
            Message *m = &b.message;
            printf("PARENT receive=%d size=%u id=%d bits=%u exact_payload=%d\n", result, m->head.msgh_size, m->head.msgh_id, m->head.msgh_bits, m->payload == sent.payload);
            if (!observed && m->head.msgh_size == sizeof(Message) && m->head.msgh_id == 901 && !(m->head.msgh_bits & MACH_MSGH_BITS_COMPLEX) && MACH_MSGH_BITS_REMOTE(m->head.msgh_bits) == MACH_MSG_TYPE_PORT_SEND_ONCE && m->payload == sent.payload) {
                observed = 1; m->head.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_MOVE_SEND_ONCE, 0); m->head.msgh_local_port = MACH_PORT_NULL; m->head.msgh_id = 902; m->payload ^= UINT64_C(0xffff);
                result = mach_msg(&m->head, MACH_SEND_MSG | MACH_SEND_TIMEOUT, sizeof(Message), 0, MACH_PORT_NULL, 500, MACH_PORT_NULL);
                printf("PARENT reply=%d\n", result);
                if (result) { mach_msg_destroy(&m->head); break; }
            } else { mach_msg_destroy(&m->head); break; }
        } else if (result != MACH_RCV_TIMED_OUT) { printf("PARENT receive_error=%d\n", result); break; }
        pid_t state = waitpid(child, &status, WNOHANG);
        if (state == child) { done = 1; break; }
        if (state < 0 && errno != EINTR) { done = -1; break; }
    }
    if (!done) { (void)kill(child, SIGKILL); done = reap(child, now() + 2, &status); }
    printf("RESULT received=%d restored=%d reaped=%d status=%d\n", observed, restored, done, status);
    // All rights and bounded lookup arrays belong to this short-lived coordinator.
    return observed && !restored && done == 1 && WIFEXITED(status) && !WEXITSTATUS(status) ? 0 : 77;
}
