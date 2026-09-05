/* Disposable self-only signature-effect calibration; no product code. */
#include <errno.h>
#include <mach/mach.h>
#include <mach/ndr.h>
#include <mach/task_special_ports.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>
#include "nonce.h"
extern int csops(pid_t, unsigned int, void *, size_t);
typedef struct { mach_msg_header_t header; NDR_record_t ndr; int32_t pid; } Request;
typedef struct { mach_msg_header_t header; NDR_record_t ndr; kern_return_t status; } Reply;
static int state(const char *stage) {
    unsigned int flags=0; unsigned char hash[20]={0};
    if(csops(getpid(),0,&flags,sizeof flags)) return 77;
    errno=0;int result=csops(getpid(),5,hash,sizeof hash),error=errno;
    printf("STATE stage=%s flags=%u hash_status=%d hash_errno=%d hash=",stage,flags,result,error);
    for(unsigned i=0;i<sizeof hash;i++) printf("%02x",hash[i]);
    putchar('\n');return 0;
}
static int lookup(void) {
    mach_port_t endpoint=MACH_PORT_NULL,reply=MACH_PORT_NULL;int outcome=77;
    if(task_get_special_port(mach_task_self(),TASK_ACCESS_PORT,&endpoint)||!MACH_PORT_VALID(endpoint)) goto done;
    if(mach_port_allocate(mach_task_self(),MACH_PORT_RIGHT_RECEIVE,&reply)) goto done;
    Request request={0};request.header.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,MACH_MSG_TYPE_MAKE_SEND_ONCE);
    request.header.msgh_remote_port=endpoint;request.header.msgh_local_port=reply;
    request.header.msgh_id=27001;request.header.msgh_size=sizeof request;request.ndr=NDR_record;request.pid=getpid();
    mach_msg_return_t sent=mach_msg(&request.header,MACH_SEND_MSG|MACH_SEND_TIMEOUT|MACH_SEND_FILTER_NONFATAL,sizeof request,0,MACH_PORT_NULL,200,MACH_PORT_NULL);
    printf("RPC self_only=1 send=%u\n",(unsigned)sent);if(sent) goto done;
    union { mach_msg_header_t header; unsigned char bytes[1024]; } buffer={0};
    mach_msg_return_t received=mach_msg(&buffer.header,MACH_RCV_MSG|MACH_RCV_TIMEOUT,0,sizeof buffer,reply,500,MACH_PORT_NULL);
    printf("RPC receive=%u\n",(unsigned)received);if(received) goto done;
    if((buffer.header.msgh_bits&MACH_MSGH_BITS_COMPLEX)||buffer.header.msgh_id!=27101||buffer.header.msgh_size!=sizeof(Reply)) {mach_msg_destroy(&buffer.header);goto done;}
    Reply response;memcpy(&response,buffer.bytes,sizeof response);
    if(response.ndr.int_rep!=NDR_record.int_rep) goto done;
    printf("RPC service_status=%d\n",response.status);outcome=response.status==0?0:77;
 done:
    if(MACH_PORT_VALID(reply)&&mach_port_mod_refs(mach_task_self(),reply,MACH_PORT_RIGHT_RECEIVE,-1)) outcome=77;
    if(MACH_PORT_VALID(endpoint)&&mach_port_deallocate(mach_task_self(),endpoint)) outcome=77;
    return outcome;
}
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);alarm(30);
    struct rlimit core={0,0},files={8192,8192};
    if(setrlimit(RLIMIT_CORE,&core)||setrlimit(RLIMIT_FSIZE,&files)||!getuid()||getuid()!=geteuid()) return 77;
    printf("READY nonce=%s uid=%u pid=%d\n",PROBE_NONCE,(unsigned)getuid(),getpid());
    if(state("before")||getchar()!='b'||lookup()||state("baseline")) return 77;
    if(getchar()!='q'||state("available")) return 77;
    if(getchar()!='a'||lookup()||state("after")) return 77;
    return 0;
}
