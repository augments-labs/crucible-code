# windows-bootstrap-v4: disposable fresh Windows x64 VM only; no production claim.
# Independently authored. V6 proved only the actual combined-token ACL primitive.
# V4: v3 native run33927711312 proved LNK2019 memcpy in ProbeEntry. Change only /O1 to /Od.
# MSVC basis: https://learn.microsoft.com/en-us/cpp/build/reference/od-disable-debug?view=msvc-170
# /NODEFAULTLIB and the kernel32-only import oracle remain mandatory; no runtime library added.
# Trusted MSVC vctip orphan was cleaned by the VM runner in v3; host descendant extinction remains unproved.
# This probe resumes synthetic native code with final restrictions from creation.
# DLL search may use KnownDLL sections before private copies; a loader failure is evidence.
# Sources: learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order
# learn.microsoft.com/en-us/cpp/build/reference/nodefaultlib-ignore-libraries
# LPAC token oracle: github.com/googleprojectzero/sandbox-attacksurface-analysis-tools/blob/main/NtCoreLib/NtToken.cs
# No permissive thread token, ACL edits outside owned dirs, registry edits, or network operations.
# Runtime closure: concrete dumpbin imports plus explicit ntdll/KernelBase/ucrtbase seeds.
# Job extinction: official JOBOBJECT_BASIC_ACCOUNTING_INFORMATION ActiveProcesses=0 after closing process handles.
# API-set contracts are counted, not fabricated as files. Dynamic/registry/IPC dependencies remain unproved.
# Three 12-second guest deadlines; 4-process kill-on-close jobs; require external CI timeout 5 minutes.
$ErrorActionPreference = 'Stop'
$buildRoot = Join-Path ([IO.Path]::GetTempPath()) ('crucible-bootstrap-v4-' + [Guid]::NewGuid().ToString('N'))
$native = @'
#include <windows.h>
static WCHAR image[32768],command[32772];
static STARTUPINFOW si;
static PROCESS_INFORMATION pi;
static char message[80];
static void emit(const char *p) { DWORD n=0,w=0; while(p[n]) ++n; WriteFile(GetStdHandle(STD_OUTPUT_HANDLE),p,n,&w,0); }
static unsigned number(unsigned n,unsigned at) { char reverse[16]; unsigned count=0; do {reverse[count++]=(char)('0'+n%10); n/=10;} while(n); while(count) message[at++]=reverse[--count]; return at; }
static void report(int child,DWORD pid,DWORD tid) { unsigned n=0; const char *label=child?"CHILD ":"SELF "; while(label[n]) {message[n]=label[n];++n;} n=number(pid,n); message[n++]=' '; n=number(tid,n); message[n++]='\n';message[n]=0; emit(message); }
static void failed(DWORD code) { unsigned n=0; message[n++]='F';message[n++]='A';message[n++]='I';message[n++]='L';message[n++]=' ';n=number(code,n);message[n++]='\n';message[n]=0;emit(message);ExitProcess(93); }
void ProbeEntry(void) {
    WCHAR *args=GetCommandLineW(); unsigned n=0,i; DWORD got=0,code=0; char ack=0; while(args[n]) ++n;
    if(!n) ExitProcess(90);
    if(args[n-1]=='C') ExitProcess(42);
    if(args[n-1]=='E') {emit("ENTRY\n");ExitProcess(41);}
    if(args[n-1]!='P') ExitProcess(90);
    report(0,GetCurrentProcessId(),GetCurrentThreadId());
    if(!ReadFile(GetStdHandle(STD_INPUT_HANDLE),&ack,1,&got,0)||got!=1||ack!=1) ExitProcess(91);
    n=GetModuleFileNameW(0,image,32768); if(!n||n>=32768) ExitProcess(92);
    command[0]='"'; for(i=0;i<n;++i) command[i+1]=image[i]; command[n+1]='"';command[n+2]=' ';command[n+3]='C';command[n+4]=0;
    si.cb=sizeof(si);
    if(!CreateProcessW(image,command,0,0,FALSE,CREATE_SUSPENDED|CREATE_NO_WINDOW,0,0,&si,&pi)) failed(GetLastError());
    report(1,pi.dwProcessId,pi.dwThreadId);
    if(WaitForSingleObject(pi.hProcess,10000)!=WAIT_OBJECT_0) {TerminateProcess(pi.hProcess,94);WaitForSingleObject(pi.hProcess,1000);CloseHandle(pi.hThread);CloseHandle(pi.hProcess);ExitProcess(94);}
    if(!GetExitCodeProcess(pi.hProcess,&code)) code=95; CloseHandle(pi.hThread);CloseHandle(pi.hProcess);
    if(code!=42) ExitProcess(96); emit("CHILD_OK\n");ExitProcess(0);
}
'@
$source = @'
using System;
using System.IO;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Security.Cryptography;
using System.Threading;
public static class CrucibleWindowsBootstrapProbeV4 {
    const uint TOKEN_QUERY = 8, TOKEN_DUPLICATE = 2, TOKEN_IMPERSONATE = 4, TOKEN_ASSIGN_PRIMARY = 1;
    const uint FILE_READ_DATA = 1, FILE_ALL_ACCESS = 0x001F01FF;
    const uint DACL = 4, PROTECTED_DACL = 0x80000000;
    const uint CREATE_SUSPENDED = 4, EXTENDED_STARTUPINFO_PRESENT = 0x80000;
    const uint CREATE_UNICODE_ENVIRONMENT = 0x400;
    static readonly IntPtr Invalid = new IntPtr(-1);
    static string stage = "start", currentCase = "start";
    [StructLayout(LayoutKind.Sequential)] struct SidAttr { public IntPtr Sid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)] struct GroupsOne { public uint Count; public SidAttr First; }
    [StructLayout(LayoutKind.Sequential)] struct SecurityCapabilities {
        public IntPtr AppContainerSid, Capabilities; public uint CapabilityCount, Reserved;
    }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct StartupInfo {
        public uint cb; public IntPtr reserved, desktop, title;
        public uint x,y,xsize,ysize,xchars,ychars,fill,flags;
        public ushort show, reserved2; public IntPtr reserved3, input, output, error;
    }
    [StructLayout(LayoutKind.Sequential)] struct StartupInfoEx {
        public StartupInfo Startup; public IntPtr Attributes;
    }
    [StructLayout(LayoutKind.Sequential)] struct ProcessInfo {
        public IntPtr Process, Thread; public uint Pid, Tid;
    }
    [StructLayout(LayoutKind.Sequential)] struct BasicLimits {
        public long ProcessTime, JobTime; public uint Flags;
        public UIntPtr MinWorkingSet, MaxWorkingSet; public uint ActiveProcesses;
        public UIntPtr Affinity; public uint Priority, Scheduling;
    }
    [StructLayout(LayoutKind.Sequential)] struct IoCounters { public ulong a,b,c,d,e,f; }
    [StructLayout(LayoutKind.Sequential)] struct ExtendedLimits {
        public BasicLimits Basic; public IoCounters Io;
        public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
    }
    [StructLayout(LayoutKind.Sequential)] struct NativeName {
        public ushort Length, MaximumLength; public IntPtr Buffer;
    }
    [StructLayout(LayoutKind.Sequential)] struct TokenAttribute {
        public NativeName Name; public ushort ValueType, Reserved;
        public uint Flags, ValueCount; public IntPtr Values;
    }
    [StructLayout(LayoutKind.Sequential)] struct AttributeHeader {
        public ushort Version, Reserved; public uint Count; public IntPtr Attributes;
    }
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern uint GetSystemDirectoryW(System.Text.StringBuilder buffer,uint size);
    [DllImport("shell32.dll", ExactSpelling=true)] static extern int SHGetKnownFolderPath(ref Guid id,uint flags,IntPtr token,out IntPtr path);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
    [DllImport("advapi32.dll")] static extern IntPtr FreeSid(IntPtr p);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr p,uint access,out IntPtr t);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool GetTokenInformation(IntPtr t,int cls,IntPtr b,uint n,out uint needed);
    [DllImport("ntdll.dll", ExactSpelling=true)] static extern uint NtQueryInformationToken(IntPtr t,int cls,IntPtr b,uint n,out uint needed);
    [DllImport("ntdll.dll", ExactSpelling=true)] static extern uint RtlNtStatusToDosError(uint status);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool CreateRestrictedToken(IntPtr t,uint flags,uint nd,IntPtr ds,uint np,IntPtr ps,uint nr,IntPtr rs,out IntPtr result);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool IsTokenRestricted(IntPtr t);
    [DllImport("advapi32.dll")] static extern bool EqualSid(IntPtr a,IntPtr b);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool ConvertStringSidToSidW(string text,out IntPtr sid);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string s,uint rev,out IntPtr sd,out uint size);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool GetSecurityDescriptorDacl(IntPtr sd,out bool present,out IntPtr dacl,out bool defaulted);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode)] static extern uint SetNamedSecurityInfoW(string name,int type,uint info,IntPtr owner,IntPtr group,IntPtr dacl,IntPtr sacl);
    [DllImport("userenv.dll", CharSet=CharSet.Unicode)] static extern int CreateAppContainerProfile(string name,string display,string description,IntPtr caps,uint count,out IntPtr sid);
    [DllImport("userenv.dll", CharSet=CharSet.Unicode)] static extern int DeleteAppContainerProfile(string name);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true, ExactSpelling=true)] static extern int CreateProcessAsUserW(IntPtr token,string app,System.Text.StringBuilder line,IntPtr processSa,IntPtr threadSa,bool inherit,uint flags,IntPtr env,string cwd,ref StartupInfoEx si,out ProcessInfo pi);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool InitializeProcThreadAttributeList(IntPtr list,int count,uint flags,ref UIntPtr size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool UpdateProcThreadAttribute(IntPtr list,uint flags,UIntPtr attr,IntPtr value,UIntPtr size,IntPtr previous,IntPtr returned);
    [DllImport("kernel32.dll")] static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateJobObjectW(IntPtr sa,string name);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr h,int cls,ref ExtendedLimits limits,uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr h,int cls,IntPtr data,uint length,out uint returned);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool IsProcessInJob(IntPtr p,IntPtr job,out bool result);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr job,uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateProcess(IntPtr p,uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern uint WaitForSingleObject(IntPtr h,uint millis);
    sealed class ProbeFailure : Exception {
        public readonly uint Code; public ProbeFailure(uint code) { Code=code; }
    }
    static void Need(bool ok) { if(!ok) throw new ProbeFailure(unchecked((uint)Marshal.GetLastWin32Error())); }
    static void Status(uint code) { if(code!=0) throw new ProbeFailure(code); }
    static string B(bool b) { return b ? "true" : "false"; }
    static void Record(string json) {
        Console.WriteLine("{\"case\":\""+currentCase+"\","+json.Substring(1));
    }
    static void VerifyLayouts() {
        bool x64=IntPtr.Size==8;
        bool valid=Marshal.SizeOf(typeof(StartupInfo))==(x64?104:68)
            && Marshal.SizeOf(typeof(StartupInfoEx))==(x64?112:72)
            && (int)Marshal.OffsetOf(typeof(StartupInfoEx),"Attributes")== (x64?104:68)
            && Marshal.SizeOf(typeof(SecurityCapabilities))==(x64?24:16)
            && Marshal.SizeOf(typeof(SidAttr))==(x64?16:8)
            && (int)Marshal.OffsetOf(typeof(GroupsOne),"First")== (x64?8:4)
            && Marshal.SizeOf(typeof(BasicLimits))==(x64?64:48)
            && Marshal.SizeOf(typeof(ExtendedLimits))==(x64?144:112)
            && Marshal.SizeOf(typeof(ProcessInfo))==(x64?24:16)
            && Marshal.SizeOf(typeof(NativeName))==(x64?16:8)
            && Marshal.SizeOf(typeof(TokenAttribute))==(x64?40:24)
            && Marshal.SizeOf(typeof(AttributeHeader))==(x64?16:12);
        Record("{\"event\":\"abi_layout\",\"matches_windows_sdk\":"+B(valid)+",\"host_64_bit\":"+B(x64)+"}");
        if(!valid) throw new ProbeFailure(13);
    }
    static void Event(string label,uint code) {
        Record("{\"event\":\""+label+"\",\"stage\":\""+stage+"\",\"status\":"+code+"}");
    }
    static void Close(ref IntPtr h) { if(h!=IntPtr.Zero) { CloseHandle(h); h=IntPtr.Zero; } }
    static IntPtr Info(IntPtr token,int cls) {
        uint needed; GetTokenInformation(token,cls,IntPtr.Zero,0,out needed);
        uint queryError=unchecked((uint)Marshal.GetLastWin32Error());
        if(needed==0 || needed>65536) {
            Record("{\"event\":\"token_info_size_failed\",\"information_class\":"+cls+",\"status\":"+queryError+"}");
            throw new ProbeFailure(queryError==0?13:queryError);
        }
        IntPtr p=Marshal.AllocHGlobal((int)needed);
        try { Need(GetTokenInformation(token,cls,p,needed,out needed)); return p; }
        catch { Marshal.FreeHGlobal(p); throw; }
    }
    static bool BoolInfo(IntPtr token,int cls) {
        IntPtr p=Info(token,cls); try { return Marshal.ReadInt32(p)!=0; }
        finally { Marshal.FreeHGlobal(p); }
    }
    static bool InQuery(IntPtr buffer,uint length,IntPtr pointer,uint bytes) {
        ulong begin=unchecked((ulong)buffer.ToInt64()),address=unchecked((ulong)pointer.ToInt64());
        return pointer!=IntPtr.Zero && address>=begin && bytes<=length && address-begin<=length-bytes;
    }
    static bool LpacInfo(IntPtr token) {
        IntPtr buffer=Marshal.AllocHGlobal(65536);
        try {
            uint used;
            uint ntStatus=NtQueryInformationToken(token,39,buffer,65536,out used);
            uint error=ntStatus==0?0:RtlNtStatusToDosError(ntStatus);
            Record("{\"event\":\"lpac_attribute_query\",\"query_success\":"+B(ntStatus==0)+",\"status\":"+error+"}");
            if(ntStatus!=0) throw new ProbeFailure(error);
            uint headerSize=(uint)Marshal.SizeOf(typeof(AttributeHeader));
            uint entrySize=(uint)Marshal.SizeOf(typeof(TokenAttribute));
            if(used<headerSize || used>65536) throw new ProbeFailure(13);
            AttributeHeader header=(AttributeHeader)Marshal.PtrToStructure(buffer,typeof(AttributeHeader));
            if(header.Version!=1 || header.Count>128) throw new ProbeFailure(13);
            if(header.Count!=0 && !InQuery(buffer,used,header.Attributes,header.Count*entrySize)) throw new ProbeFailure(13);
            bool present=false,typed=false,single=false,enabled=false,nonzero=false;
            for(uint i=0;i<header.Count;i++) {
                IntPtr entry=IntPtr.Add(header.Attributes,(int)(i*entrySize));
                TokenAttribute attribute=(TokenAttribute)Marshal.PtrToStructure(entry,typeof(TokenAttribute));
                if(attribute.Name.Length==0) continue;
                if(attribute.Name.Length>2048 || attribute.Name.Length%2!=0
                    || attribute.Name.MaximumLength<attribute.Name.Length
                    || !InQuery(buffer,used,attribute.Name.Buffer,attribute.Name.Length)) throw new ProbeFailure(13);
                string name=Marshal.PtrToStringUni(attribute.Name.Buffer,attribute.Name.Length/2);
                if(!String.Equals(name,"WIN://NOALLAPPPKG",StringComparison.OrdinalIgnoreCase)) continue;
                if(present) throw new ProbeFailure(13);
                present=true; typed=attribute.ValueType==2; single=attribute.ValueCount==1;
                enabled=(attribute.Flags & 0x54)==0; // not deny-only, disabled, or ignored
                if(typed && single) {
                    if(!InQuery(buffer,used,attribute.Values,8)) throw new ProbeFailure(13);
                    nonzero=Marshal.ReadInt64(attribute.Values)!=0;
                }
            }
            Record("{\"event\":\"lpac_security_attribute\",\"no_all_app_pkg_present\":"+B(present)+",\"uint64_type\":"+B(typed)+",\"single_value\":"+B(single)+",\"enabled\":"+B(enabled)+",\"nonzero\":"+B(nonzero)+"}");
            return present && typed && single && enabled && nonzero;
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    static bool OnlySid(IntPtr token,IntPtr expected) {
        IntPtr p=Info(token,11);
        try {
            int offset=(int)Marshal.OffsetOf(typeof(GroupsOne),"First");
            return Marshal.ReadInt32(p)==1 && EqualSid(Marshal.ReadIntPtr(p,offset),expected);
        } finally { Marshal.FreeHGlobal(p); }
    }
    static string RandomSid() {
        byte[] bytes=new byte[16]; using(RandomNumberGenerator r=RandomNumberGenerator.Create()) { r.GetBytes(bytes); }
        return "S-1-5-21-"+BitConverter.ToUInt32(bytes,0)+"-"+BitConverter.ToUInt32(bytes,4)+"-"+BitConverter.ToUInt32(bytes,8)+"-"+BitConverter.ToUInt32(bytes,12);
    }
    static string SidText(IntPtr sid) { return new SecurityIdentifier(sid).Value; }
    static void Acl(string path,string user,string extra) {
        IntPtr sd=IntPtr.Zero; uint size;
        try {
            Need(ConvertStringSecurityDescriptorToSecurityDescriptorW("D:P(A;;FA;;;"+user+")"+extra,1,out sd,out size));
            bool present,defaulted; IntPtr dacl;
            Need(GetSecurityDescriptorDacl(sd,out present,out dacl,out defaulted));
            if(!present) throw new ProbeFailure(13);
            Status(SetNamedSecurityInfoW(path,1,DACL|PROTECTED_DACL,IntPtr.Zero,IntPtr.Zero,dacl,IntPtr.Zero));
        } finally { if(sd!=IntPtr.Zero) LocalFree(sd); }
    }
    static string Grant(string sid) { return "(A;;GRGX;;;"+sid+")"; }
    static void Attribute(IntPtr list,uint attr,IntPtr value,int size) {
        Need(UpdateProcThreadAttribute(list,0,new UIntPtr(attr),value,new UIntPtr((uint)size),IntPtr.Zero,IntPtr.Zero));
    }
    static bool EmptyJob(IntPtr job) {
        IntPtr data=Marshal.AllocHGlobal(48);
        try {
            uint returned;
            Need(QueryInformationJobObject(job,1,data,48,out returned));
            return returned==48 && Marshal.ReadInt32(data,40)==0;
        } finally { Marshal.FreeHGlobal(data); }
    }
    [StructLayout(LayoutKind.Sequential)] struct Sa { public uint Length; public IntPtr Descriptor; public int Inherit; }
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool CreatePipe(out IntPtr read,out IntPtr write,ref Sa sa,uint size);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetHandleInformation(IntPtr h,uint mask,uint flags);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool PeekNamedPipe(IntPtr h,IntPtr b,uint n,IntPtr read,out uint available,IntPtr left);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool ReadFile(IntPtr h,byte[] b,uint n,out uint read,IntPtr ov);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool WriteFile(IntPtr h,byte[] b,uint n,out uint wrote,IntPtr ov);
    [DllImport("kernel32.dll",SetLastError=true)] static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr process,out uint code);
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenThread(uint access,bool inherit,uint tid);
    [DllImport("kernel32.dll",SetLastError=true)] static extern uint GetProcessIdOfThread(IntPtr thread);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenThreadToken(IntPtr thread,uint access,bool self,out IntPtr token);
    static string Digest(string file) { using(SHA256 h=SHA256.Create()) using(FileStream f=File.OpenRead(file)) return BitConverter.ToString(h.ComputeHash(f)).Replace("-","").ToLowerInvariant(); }
    static void Audit(IntPtr process,IntPtr thread,IntPtr job,IntPtr package,IntPtr session,string label) {
        IntPtr token=IntPtr.Zero,imp=IntPtr.Zero,caps=IntPtr.Zero,pkg=IntPtr.Zero;
        try {
            bool inJob; Need(IsProcessInJob(process,job,out inJob)); Need(OpenProcessToken(process,TOKEN_QUERY,out token));
            bool tokenShape=BoolInfo(token,29)&&LpacInfo(token)&&IsTokenRestricted(token)&&OnlySid(token,session);
            caps=Info(token,30); pkg=Info(token,31);
            tokenShape=tokenShape&&Marshal.ReadInt32(caps)==0&&Marshal.ReadIntPtr(pkg)!=IntPtr.Zero&&EqualSid(Marshal.ReadIntPtr(pkg),package);
            bool hasThreadToken=OpenThreadToken(thread,TOKEN_QUERY,true,out imp);
            uint error=unchecked((uint)Marshal.GetLastWin32Error()); bool absent=!hasThreadToken&&error==1008;
            Record("{\"event\":\"actual_execution_token\",\"fixture\":\""+label+"\",\"lpac_zero_capabilities_exact_session_sid\":"+B(tokenShape)+",\"read_restriction_retested\":false,\"in_job\":"+B(inJob)+",\"thread_token_absent\":"+B(absent)+",\"thread_query_status\":"+(hasThreadToken?0:error)+"}");
            if(!tokenShape||!inJob||!absent) throw new ProbeFailure(13);
        } finally { Close(ref token); Close(ref imp); if(caps!=IntPtr.Zero) Marshal.FreeHGlobal(caps); if(pkg!=IntPtr.Zero) Marshal.FreeHGlobal(pkg); }
    }
    static void Observe(string line,IntPtr input,IntPtr job,IntPtr package,IntPtr session,ref bool self,ref bool child,ref IntPtr descendant) {
        string[] parts=line.Split(' '); if(parts.Length!=3) throw new ProbeFailure(13);
        uint pid,tid; if(!UInt32.TryParse(parts[1],out pid)||!UInt32.TryParse(parts[2],out tid)) throw new ProbeFailure(13);
        bool suspended=parts[0]=="CHILD";
        if((suspended&&child)||(!suspended&&(parts[0]!="SELF"||self))) throw new ProbeFailure(13);
        IntPtr process=IntPtr.Zero,thread=IntPtr.Zero;
        try {
            process=OpenProcess(0x1000|0x100000,false,pid); Need(process!=IntPtr.Zero);
            thread=OpenThread(0x40|2,false,tid); Need(thread!=IntPtr.Zero);
            if(GetProcessIdOfThread(thread)!=pid) throw new ProbeFailure(13);
            Audit(process,thread,job,package,session,suspended?"native_child_before_resume":"native_parent_at_handshake");
            if(suspended) { if(ResumeThread(thread)!=1) throw new ProbeFailure(13); child=true; descendant=process; process=IntPtr.Zero; }
            else { uint wrote; Need(WriteFile(input,new byte[]{1},1,out wrote,IntPtr.Zero)); if(wrote!=1) throw new ProbeFailure(13); self=true; }
        } finally { Close(ref thread); Close(ref process); }
    }
    static bool Execute(string name,string application,string command,string root,string env,IntPtr restricted,IntPtr package,IntPtr session,out bool cleaned) {
        currentCase=name; stage="job_create"; cleaned=false; bool success=false,listReady=false,spawned=false;
        IntPtr job=IntPtr.Zero,list=IntPtr.Zero,cap=IntPtr.Zero,lpac=IntPtr.Zero,jobs=IntPtr.Zero,handles=IntPtr.Zero,environment=IntPtr.Zero;
        IntPtr inputRead=IntPtr.Zero,inputWrite=IntPtr.Zero,outputRead=IntPtr.Zero,outputWrite=IntPtr.Zero,descendant=IntPtr.Zero;
        ProcessInfo pi=new ProcessInfo();
        try {
            job=CreateJobObjectW(IntPtr.Zero,null); Need(job!=IntPtr.Zero);
            ExtendedLimits limits=new ExtendedLimits(); limits.Basic.Flags=0x2000|8; limits.Basic.ActiveProcesses=4;
            Need(SetInformationJobObject(job,9,ref limits,(uint)Marshal.SizeOf(typeof(ExtendedLimits))));
            stage="owned_stdio"; Sa sa=new Sa { Length=(uint)Marshal.SizeOf(typeof(Sa)),Inherit=1 };
            Need(CreatePipe(out inputRead,out inputWrite,ref sa,4096)); Need(CreatePipe(out outputRead,out outputWrite,ref sa,4096));
            Need(SetHandleInformation(inputWrite,1,0)); Need(SetHandleInformation(outputRead,1,0));
            stage="final_attributes"; UIntPtr bytes=UIntPtr.Zero; InitializeProcThreadAttributeList(IntPtr.Zero,4,0,ref bytes);
            if(bytes.ToUInt64()==0||bytes.ToUInt64()>65536) throw new ProbeFailure(13);
            list=Marshal.AllocHGlobal((int)bytes.ToUInt64()); Need(InitializeProcThreadAttributeList(list,4,0,ref bytes)); listReady=true;
            cap=Marshal.AllocHGlobal(Marshal.SizeOf(typeof(SecurityCapabilities))); Marshal.StructureToPtr(new SecurityCapabilities {AppContainerSid=package},cap,false);
            lpac=Marshal.AllocHGlobal(4); Marshal.WriteInt32(lpac,1); jobs=Marshal.AllocHGlobal(IntPtr.Size); Marshal.WriteIntPtr(jobs,job);
            handles=Marshal.AllocHGlobal(2*IntPtr.Size); Marshal.WriteIntPtr(handles,inputRead); Marshal.WriteIntPtr(handles,IntPtr.Size,outputWrite);
            Attribute(list,0x20009,cap,Marshal.SizeOf(typeof(SecurityCapabilities))); Attribute(list,0x2000F,lpac,4);
            Attribute(list,0x2000D,jobs,IntPtr.Size); Attribute(list,0x20002,handles,2*IntPtr.Size);
            StartupInfoEx si=new StartupInfoEx(); si.Startup.cb=(uint)Marshal.SizeOf(typeof(StartupInfoEx)); si.Attributes=list;
            si.Startup.flags=0x100; si.Startup.input=inputRead; si.Startup.output=outputWrite; si.Startup.error=outputWrite;
            environment=Marshal.StringToHGlobalUni(env); stage="create_suspended";
            int launch=CreateProcessAsUserW(restricted,application,new System.Text.StringBuilder(command),IntPtr.Zero,IntPtr.Zero,true,
                CREATE_SUSPENDED|EXTENDED_STARTUPINFO_PRESENT|CREATE_UNICODE_ENVIRONMENT|0x08000000,environment,root,ref si,out pi);
            uint error=unchecked((uint)Marshal.GetLastWin32Error()); spawned=launch!=0;
            Record("{\"event\":\"launch_result\",\"success\":"+B(spawned)+",\"status\":"+(spawned?0:error)+"}"); if(!spawned) throw new ProbeFailure(error);
            Close(ref inputRead); Close(ref outputWrite); stage="before_first_instruction";
            Audit(pi.Process,pi.Thread,job,package,session,"root_before_resume");
            if(ResumeThread(pi.Thread)!=1) throw new ProbeFailure(13);
            stage="guest_execution"; Stopwatch deadline=Stopwatch.StartNew(); byte[] buffer=new byte[4096]; string pending="";
            int total=0; bool entry=false,self=false,child=false,childOk=false,cmdReady=false,cmdOk=false;
            while(true) {
                if(deadline.ElapsedMilliseconds>12000) throw new ProbeFailure(1460);
                uint available; bool peek=PeekNamedPipe(outputRead,IntPtr.Zero,0,IntPtr.Zero,out available,IntPtr.Zero);
                if(!peek) { uint e=unchecked((uint)Marshal.GetLastWin32Error()); if(e!=109) throw new ProbeFailure(e); available=0; }
                if(available!=0) {
                    uint read; Need(ReadFile(outputRead,buffer,Math.Min(available,(uint)buffer.Length),out read,IntPtr.Zero));
                    total+=(int)read; if(total>32768) throw new ProbeFailure(122); pending+=System.Text.Encoding.ASCII.GetString(buffer,0,(int)read);
                    int newline; while((newline=pending.IndexOf('\n'))>=0) {
                        string line=pending.Substring(0,newline).TrimEnd('\r'); pending=pending.Substring(newline+1);
                        if(line=="ENTRY") entry=true; else if(line=="CHILD_OK") childOk=true; else if(line=="CMD_READY") cmdReady=true; else if(line=="CMD_OK") cmdOk=true;
                        else if(line.StartsWith("FAIL ")) {uint errorCode;if(!UInt32.TryParse(line.Substring(5),out errorCode)) throw new ProbeFailure(13);Event("native_child_create_failure",errorCode);}
                        else if(line.StartsWith("SELF ")||line.StartsWith("CHILD ")) Observe(line,inputWrite,job,package,session,ref self,ref child,ref descendant);
                        else throw new ProbeFailure(13);
                    }
                    if(pending.Length>1024) throw new ProbeFailure(122);
                } else if(WaitForSingleObject(pi.Process,0)==0) break; else Thread.Sleep(10);
            }
            uint exitCode; Need(GetExitCodeProcess(pi.Process,out exitCode)); uint descendantCode=0;
            if(descendant!=IntPtr.Zero) { if(WaitForSingleObject(descendant,0)!=0) throw new ProbeFailure(13); Need(GetExitCodeProcess(descendant,out descendantCode)); }
            success=pending.Length==0&&(name=="native_entry" ? entry&&exitCode==41 : self&&child&&childOk&&descendantCode==42&&exitCode==0&&(name!="staged_cmd"||cmdReady&&cmdOk));
            Record("{\"event\":\"bootstrap_result\",\"entry_marker\":"+B(entry)+",\"child_audited\":"+B(child)+",\"child_completed\":"+B(childOk&&descendantCode==42)+",\"cmd_markers\":"+B(cmdReady&&cmdOk)+",\"exit_status\":"+exitCode+",\"pass\":"+B(success)+"}");
        } catch(ProbeFailure f) { Event("probe_api_failure",f.Code); } catch(Exception) { Event("probe_managed_failure",0); }
        finally {
            stage="scope_stop"; bool empty=!spawned;
            if(spawned) {
                TerminateJobObject(job,1); TerminateProcess(pi.Process,1); uint waited=WaitForSingleObject(pi.Process,5000);
                Close(ref descendant); Close(ref pi.Thread); Close(ref pi.Process);
                Stopwatch deadline=Stopwatch.StartNew(); try { while(deadline.ElapsedMilliseconds<5000) { if(EmptyJob(job)) {empty=waited==0; break;} Thread.Sleep(25); } } catch(Exception) { empty=false; }
            }
            Record("{\"event\":\"job_extinction\",\"empty\":"+B(empty)+"}"); cleaned=empty;
            Close(ref descendant); Close(ref pi.Thread); Close(ref pi.Process); Close(ref inputRead); Close(ref inputWrite); Close(ref outputRead); Close(ref outputWrite); Close(ref job);
            if(listReady) DeleteProcThreadAttributeList(list);
            foreach(IntPtr p in new IntPtr[]{list,cap,lpac,jobs,handles,environment}) if(p!=IntPtr.Zero) Marshal.FreeHGlobal(p);
        }
        return success&&cleaned;
    }
    public static int Run(string buildRoot,string[] sourceFiles,string compilerIdentity) {
        string profile="crucible.bootstrap.v4."+Guid.NewGuid().ToString("N"),root=null; bool profileOwned=false,rootOwned=false,cleanup=true; int result=1;
        IntPtr package=IntPtr.Zero,session=IntPtr.Zero,baseToken=IntPtr.Zero,restricted=IntPtr.Zero,entry=IntPtr.Zero;
        try {
            currentCase="setup"; VerifyLayouts(); stage="profile";
            Status(unchecked((uint)CreateAppContainerProfile(profile,"Crucible bootstrap probe","Disposable synthetic execution",IntPtr.Zero,0,out package))); profileOwned=true;
            string p=SidText(package),s=RandomSid(),user; using(WindowsIdentity identity=WindowsIdentity.GetCurrent()) user=identity.User.Value;
            Need(ConvertStringSidToSidW(s,out session)); root=Path.Combine(buildRoot,"runtime"); if(Directory.Exists(root)) throw new ProbeFailure(183);
            Directory.CreateDirectory(root); rootOwned=true; Acl(buildRoot,user,Grant(p)+Grant(s)); Acl(root,user,Grant(p)+Grant(s));
            stage="runtime_manifest"; System.Text.StringBuilder system=new System.Text.StringBuilder(32768); uint n=GetSystemDirectoryW(system,(uint)system.Capacity);
            Need(n!=0); if(n>=(uint)system.Capacity) throw new ProbeFailure(122); string sys=system.ToString(),win=Path.GetDirectoryName(sys);
            if(sourceFiles.Length<2||sourceFiles.Length>64) throw new ProbeFailure(122);
            long total=0; string manifest=""; var names=new System.Collections.Generic.HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach(string source in sourceFiles) {
                string name=Path.GetFileName(source),parent=Path.GetDirectoryName(Path.GetFullPath(source)); bool fixture=name=="fixture.exe"&&String.Equals(parent,buildRoot,StringComparison.OrdinalIgnoreCase);
                if(!fixture&&!String.Equals(parent,sys,StringComparison.OrdinalIgnoreCase)) throw new ProbeFailure(13);
                if(!System.Text.RegularExpressions.Regex.IsMatch(name,@"\A[A-Za-z0-9_.-]+\z")||!names.Add(name)) throw new ProbeFailure(13);
                if((File.GetAttributes(source)&FileAttributes.ReparsePoint)!=0) throw new ProbeFailure(13);
                long length=new FileInfo(source).Length; total+=length; if(length<=0||total>134217728) throw new ProbeFailure(122);
                string destination=Path.Combine(root,name),hash=Digest(source); File.Copy(source,destination,false); if(Digest(destination)!=hash) throw new ProbeFailure(13);
                Acl(destination,user,Grant(p)+Grant(s)); manifest+=name+" "+length+" "+hash+"\n";
                Record("{\"event\":\"runtime_file\",\"fixture\":\""+name+"\",\"source_identity\":\""+(fixture?"compiled_fixture":"native_system_directory")+"\",\"bytes\":"+length+",\"sha256\":\""+hash+"\"}");
            }
            string manifestHash; using(SHA256 hash=SHA256.Create()) manifestHash=BitConverter.ToString(hash.ComputeHash(System.Text.Encoding.UTF8.GetBytes(manifest))).Replace("-","").ToLowerInvariant();
            Record("{\"event\":\"runtime_manifest\",\"count\":"+sourceFiles.Length+",\"bytes\":"+total+",\"sha256\":\""+manifestHash+"\",\"os_version\":\""+Environment.OSVersion.Version+"\",\"compiler_sha256\":\""+compilerIdentity+"\"}");
            stage="restricted_token"; Need(OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY|TOKEN_DUPLICATE|TOKEN_ASSIGN_PRIMARY,out baseToken)); if(IsTokenRestricted(baseToken)) throw new ProbeFailure(50);
            entry=Marshal.AllocHGlobal(Marshal.SizeOf(typeof(SidAttr))); Marshal.StructureToPtr(new SidAttr{Sid=session},entry,false);
            Need(CreateRestrictedToken(baseToken,1,0,IntPtr.Zero,0,IntPtr.Zero,1,entry,out restricted)); if(!OnlySid(restricted,session)) throw new ProbeFailure(13);
            stage="environment"; Guid id=new Guid("F1B32785-6FBA-4FCF-9D55-7B8E7F157091"); IntPtr local=IntPtr.Zero; string localPath;
            try {Status(unchecked((uint)SHGetKnownFolderPath(ref id,0,IntPtr.Zero,out local))); localPath=Marshal.PtrToStringUni(local);} finally {if(local!=IntPtr.Zero) Marshal.FreeCoTaskMem(local);}
            if(String.IsNullOrEmpty(localPath)||!Path.IsPathRooted(localPath)) throw new ProbeFailure(13);
            string exe=Path.Combine(root,"fixture.exe"),cmd=Path.Combine(root,"cmd.exe");
            string env="COMSPEC="+cmd+"\0LOCALAPPDATA="+localPath+"\0PATH="+root+"\0SystemRoot="+win+"\0WINDIR="+win+"\0";
            bool clean; if(!Execute("native_entry",exe,"\""+exe+"\" E",root,env,restricted,package,session,out clean)) {cleanup=clean; throw new ProbeFailure(13);}
            if(!Execute("native_child",exe,"\""+exe+"\" P",root,env,restricted,package,session,out clean)) {cleanup=clean; throw new ProbeFailure(13);}
            if(!Execute("staged_cmd",cmd,"\""+cmd+"\" /d /q /c \"echo CMD_READY&fixture.exe P&&echo CMD_OK\"",root,env,restricted,package,session,out clean)) {cleanup=clean; throw new ProbeFailure(13);}
            result=0;
        } catch(ProbeFailure f) {Event("probe_api_failure",f.Code);} catch(Exception) {Event("probe_managed_failure",0);}
        finally {
            Close(ref restricted); Close(ref baseToken); if(entry!=IntPtr.Zero) Marshal.FreeHGlobal(entry); if(session!=IntPtr.Zero) LocalFree(session); if(package!=IntPtr.Zero) FreeSid(package);
            if(cleanup&&rootOwned) try { Directory.Delete(root,true); } catch(Exception) {cleanup=false;}
            if(cleanup&&profileOwned) {int hr=DeleteAppContainerProfile(profile); if(hr!=0) {cleanup=false; Event("profile_cleanup_failed",unchecked((uint)hr));}}
            Record("{\"event\":\"cleanup\",\"complete\":"+B(cleanup)+"}");
        }
        if(!cleanup) result=3; Record("{\"event\":\"bootstrap_summary\",\"exit_code\":"+result+",\"ordinary_cmd_validated\":"+B(result==0)+",\"network_tested\":false}"); return result;
    }
}
'@
# Shared stdout+stderr cap is 1 Mi characters, checked before accumulation. Only capped LNK diagnostics are emitted.
# Two fixed 4096-character buffers and StreamReader's fixed decoder buffers are the only in-flight data.
# Deadline includes process exit and both EOFs. Abort never waits for cancellation/drain completion.
$hostToolUnsettled=$false
function Invoke-ProbeTool([string]$Tool,[string[]]$Arguments) {
    $info=[Diagnostics.ProcessStartInfo]::new($Tool); $info.UseShellExecute=$false; $info.RedirectStandardOutput=$true; $info.RedirectStandardError=$true
    foreach($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $process=[Diagnostics.Process]::new(); $process.StartInfo=$info; $started=$false; $complete=$false
    $cancel=[Threading.CancellationTokenSource]::new(); $deadline=[Diagnostics.Stopwatch]::StartNew()
    $readers=@(); $buffers=@([char[]]::new(4096),[char[]]::new(4096)); $tasks=@($null,$null); $eof=@($false,$false)
    $output=[Text.StringBuilder]::new(4096); $errors=[Text.StringBuilder]::new(4096); [long]$seenCharacters=0
    try {
        $started=$process.Start(); if(!$started) { throw 'tool_start' }; $readers=@($process.StandardOutput,$process.StandardError)
        for($i=0;$i -lt 2;$i++) { $tasks[$i]=$readers[$i].ReadAsync([Memory[char]]::new($buffers[$i]),$cancel.Token).AsTask() }
        while($true) {
            if($deadline.ElapsedMilliseconds -ge 20000) { throw 'tool_deadline' }
            for($i=0;$i -lt 2;$i++) {
                if(!$eof[$i] -and $tasks[$i].IsCompleted) {
                    $count=$tasks[$i].GetAwaiter().GetResult()
                    if($count -eq 0) { $eof[$i]=$true; continue }
                    if($count -lt 0 -or $count -gt 4096 -or $seenCharacters+$count -gt 1048576) { throw 'tool_output_cap' }
                    $seenCharacters+=$count; if($i -eq 0) { [void]$output.Append($buffers[$i],0,$count) } else { [void]$errors.Append($buffers[$i],0,$count) }
                    $tasks[$i]=$readers[$i].ReadAsync([Memory[char]]::new($buffers[$i]),$cancel.Token).AsTask()
                }
            }
            if($deadline.ElapsedMilliseconds -ge 20000) { throw 'tool_deadline' }
            if($process.HasExited -and $eof[0] -and $eof[1]) { $complete=$true; break }
            [Threading.Thread]::Sleep(5)
        }
        if($hostStage -in @('compile_fixture','link_fixture')) { @{event='host_tool_exit';stage=$hostStage;exit_status=$process.ExitCode;output_characters=$seenCharacters} | ConvertTo-Json -Compress | Out-Host }
        if($process.ExitCode -ne 0) {
            if($hostStage -eq 'link_fixture') {
                $text=$output.ToString()+"`n"+$errors.ToString(); $linkMatch=[regex]::Match($text,'\b(LNK[0-9]{4}):[ \t]*([\x20-\x7e]{1,480})'); $reported=0
                while($linkMatch.Success -and $reported -lt 16) {
                    @{event='link_diagnostic';code=$linkMatch.Groups[1].Value;message=$linkMatch.Groups[2].Value} | ConvertTo-Json -Compress | Out-Host
                    $reported++; $linkMatch=$linkMatch.NextMatch()
                }
                @{event='link_diagnostic_count';reported=$reported;additional_matches=$linkMatch.Success;message_character_cap=480;record_cap=16} | ConvertTo-Json -Compress | Out-Host
            }
            Write-Output ('{"event":"host_tool_failure","exit_status":'+$process.ExitCode+'}') | Out-Host; throw 'tool_failed'
        }
        return $output.ToString()
    } finally {
        if($started -and !$complete) {
            $script:hostToolUnsettled=$true; $requested=$false
            try { $cancel.Cancel() } catch {}
            try { $process.Kill($true); $requested=$true } catch {}
            Write-Output ('{"event":"host_tool_abort","tree_termination_requested":'+$requested.ToString().ToLowerInvariant()+',"scope_stop_confirmed":false}') | Out-Host
        }
        foreach($reader in $readers) { try { $reader.BaseStream.Dispose() } catch {}; try { $reader.Dispose() } catch {} }
        $cancel.Dispose(); $process.Dispose()
    }
}
$result=90; $buildOwned=$false; $hostStage="toolchain_discovery"
try {
    if([IntPtr]::Size -ne 8) { throw 'x64_required' }; if(Test-Path -LiteralPath $buildRoot) { throw 'unique_root_collision' }
    [IO.Directory]::CreateDirectory($buildRoot)|Out-Null; $buildOwned=$true
    [IO.File]::WriteAllText((Join-Path $buildRoot 'fixture.c'),$native,[Text.Encoding]::ASCII)
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vs=(Invoke-ProbeTool $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath')).Trim()
    if(!$vs -or !(Test-Path -LiteralPath $vs)) { throw 'msvc_missing' }
    $vc=(Get-ChildItem -LiteralPath (Join-Path $vs 'VC\Tools\MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1).FullName
    $kits=Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
    $sdk=(Get-ChildItem -LiteralPath (Join-Path $kits 'Include') -Directory | Where-Object {Test-Path (Join-Path $_.FullName 'um\windows.h')} | Sort-Object Name -Descending | Select-Object -First 1).Name
    $bin=Join-Path $vc 'bin\Hostx64\x64'; $cl=Join-Path $bin 'cl.exe'; $link=Join-Path $bin 'link.exe'; $dumpbin=Join-Path $bin 'dumpbin.exe'
    $object=Join-Path $buildRoot 'fixture.obj'; $fixture=Join-Path $buildRoot 'fixture.exe'
    $compile=@('/nologo','/Od','/GS-','/Zl','/c','/X',('/Fo'+$object),('/I'+(Join-Path $vc 'include')))
    foreach($part in @('um','shared','ucrt')) { $compile+=('/I'+(Join-Path $kits ('Include\'+$sdk+'\'+$part))) }; $compile+=(Join-Path $buildRoot 'fixture.c')
    $hostStage="compile_fixture"; Invoke-ProbeTool $cl $compile | Out-Null
    $hostStage="link_fixture"; Invoke-ProbeTool $link @('/NOLOGO','/NODEFAULTLIB','/ENTRY:ProbeEntry','/SUBSYSTEM:CONSOLE','/MACHINE:X64','/DYNAMICBASE','/NXCOMPAT','/MANIFEST:NO',('/OUT:'+$fixture),$object,(Join-Path $kits ('Lib\'+$sdk+'\um\x64\kernel32.lib'))) | Out-Null
    $hostStage="dependency_manifest"; $system=[Environment]::SystemDirectory; $queue=[Collections.Generic.Queue[string]]::new(); $queue.Enqueue($fixture)
    foreach($name in @('cmd.exe','ntdll.dll','kernel32.dll','KernelBase.dll','ucrtbase.dll')) { $queue.Enqueue((Join-Path $system $name)) }
    $files=[Collections.Generic.Dictionary[string,string]]::new([StringComparer]::OrdinalIgnoreCase); $contracts=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase); [long]$bytes=0
    while($queue.Count) {
        $file=$queue.Dequeue(); $name=[IO.Path]::GetFileName($file); if($files.ContainsKey($name)) {continue}
        $item=Get-Item -LiteralPath $file; if($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {throw 'source_reparse'}
        $bytes+=$item.Length; if($files.Count -ge 64 -or $bytes -gt 134217728) {throw 'runtime_bound'}; $files.Add($name,$file)
        $imports=Invoke-ProbeTool $dumpbin @('/NOLOGO','/DEPENDENTS',$file)
        if($name -eq 'fixture.exe' -and $imports -notmatch '(?im)^\s+kernel32\.dll\s*$') {throw 'fixture_import_missing'}
        foreach($match in [regex]::Matches($imports,'(?im)^\s+([a-z0-9_.-]+\.dll)\s*$')) {
            $dependency=$match.Groups[1].Value
            if($name -eq 'fixture.exe' -and $dependency -ine 'kernel32.dll') {throw 'fixture_import_not_kernel32'}
            if($dependency -match '^(api-ms-|ext-ms-)') { [void]$contracts.Add($dependency); continue }
            $queue.Enqueue((Join-Path $system $dependency))
        }
    }
    Write-Output ('{"event":"host_compile","success":true,"api_set_contract_count":'+$contracts.Count+'}')
    $hostStage="compile_host"; Add-Type -TypeDefinition $source -Language CSharp -ErrorAction Stop
    [string[]]$sources=@($files.Values | Sort-Object { [IO.Path]::GetFileName($_).ToLowerInvariant() })
    $hostStage="invoke_host"; $result=[CrucibleWindowsBootstrapProbeV4]::Run($buildRoot,$sources,(Get-FileHash -LiteralPath $cl -Algorithm SHA256).Hash.ToLowerInvariant())
} catch { Write-Output ('{"event":"host_setup_failure","stage":"'+$hostStage+'","hresult":'+$_.Exception.HResult+'}'); $result=90 }
finally { if($hostToolUnsettled) { $result=3 }; if($buildOwned -and $result -ne 3) { try { Remove-Item -LiteralPath $buildRoot -Recurse -Force -ErrorAction Stop } catch { Write-Output '{"event":"build_cleanup","complete":false}'; $result=3 } } }
exit $result
