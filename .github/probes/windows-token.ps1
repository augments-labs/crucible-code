# Probe identity: windows-token-v5; immutable predecessors retained.
# V4 run33924136290 confirms LOCALAPPDATA setup; the null-buffer sizing call
# for token class46 fails87. This does not test a correctly sized DWORD call.
# SDK audit: TokenIsLessPrivilegedAppContainer=46; TokenProcessTrustLevel=41.
# https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/winnt.h
# https://github.com/microsoft/WindowsAppSDK/discussions/3633
# https://github.com/winsiderss/phnt/blob/master/ntseapi.h (class46 queries ULONG)
# H6: direct fixed-DWORD query succeeds where generic zero-length sizing fails.
# Independently query Win32 and Nt APIs; successful answers must agree.
# At least one correctly sized query must affirm LPAC, for child AND duplicate.
# Query failure never becomes an assumed LPAC flag or weaker launch.
# Same four controls, environment, token restrictions, matrix and cleanup as v4.
# Disposable experiment, not a production launcher. Owns only its unique temp
# directory and current-user AppContainer profile. Run in a disposable Windows
# runner with a job-level timeout (suggested: 3 minutes). No network operations.
# Question: does LPAC creation preserve an all-access restricting SID, and do
# actual file opens require both that SID and an AppContainer grant?
# Guest: trusted System32 cmd.exe, created suspended and NEVER resumed.
# No permissive bootstrap token, fallback launch, account setup, or runtime ACL edits.
# Sources (architecture/API only; implementation independently written):
# https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken
# https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer
# https://chromium.googlesource.com/chromium/src/+/main/sandbox/win/src/app_container_test.cc
# https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemdirectoryw
# https://docs.python.org/3/library/subprocess.html (SystemRoot/SxS requirement)
# Limits: tests file access using an impersonation duplicate of the real suspended
# child's token; does not establish guest bootstrap, descendant, or network behavior.
# All denied rows must fail CreateFile with ERROR_ACCESS_DENIED (5), not a missing
# file error. A readable NULL DACL is an exact-root counterexample, not a pass.
$ErrorActionPreference = 'Stop'
$source = @'
using System;
using System.IO;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Security.Cryptography;
using System.Threading;

public static class CrucibleWindowsTokenProbeV5 {
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
    [StructLayout(LayoutKind.Sequential)] struct Mapping { public uint Read, Write, Execute, All; }

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
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool DuplicateTokenEx(IntPtr t,uint access,IntPtr sa,int level,int type,out IntPtr result);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool ImpersonateLoggedOnUser(IntPtr t);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool RevertToSelf();
    [DllImport("advapi32.dll")] static extern bool EqualSid(IntPtr a,IntPtr b);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool ConvertStringSidToSidW(string text,out IntPtr sid);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string s,uint rev,out IntPtr sd,out uint size);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool GetSecurityDescriptorDacl(IntPtr sd,out bool present,out IntPtr dacl,out bool defaulted);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode)] static extern uint SetNamedSecurityInfoW(string name,int type,uint info,IntPtr owner,IntPtr group,IntPtr dacl,IntPtr sacl);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode)] static extern uint GetNamedSecurityInfoW(string name,int type,uint info,out IntPtr owner,out IntPtr group,out IntPtr dacl,out IntPtr sacl,out IntPtr sd);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool AccessCheck(IntPtr sd,IntPtr token,uint access,ref Mapping mapping,IntPtr privileges,ref uint length,out uint granted,out bool allowed);
    [DllImport("userenv.dll", CharSet=CharSet.Unicode)] static extern int CreateAppContainerProfile(string name,string display,string description,IntPtr caps,uint count,out IntPtr sid);
    [DllImport("userenv.dll", CharSet=CharSet.Unicode)] static extern int DeleteAppContainerProfile(string name);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true, ExactSpelling=true)] static extern int CreateProcessAsUserW(IntPtr token,string app,System.Text.StringBuilder line,IntPtr processSa,IntPtr threadSa,bool inherit,uint flags,IntPtr env,string cwd,ref StartupInfoEx si,out ProcessInfo pi);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool InitializeProcThreadAttributeList(IntPtr list,int count,uint flags,ref UIntPtr size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool UpdateProcThreadAttribute(IntPtr list,uint flags,UIntPtr attr,IntPtr value,UIntPtr size,IntPtr previous,IntPtr returned);
    [DllImport("kernel32.dll")] static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateJobObjectW(IntPtr sa,string name);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr h,int cls,ref ExtendedLimits limits,uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr h,int cls,IntPtr data,uint length,out uint returned);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool IsProcessInJob(IntPtr p,IntPtr job,out bool result);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr job,uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateProcess(IntPtr p,uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern uint WaitForSingleObject(IntPtr h,uint millis);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateFileW(string path,uint access,uint share,IntPtr sa,uint disposition,uint flags,IntPtr template);

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
            && Marshal.SizeOf(typeof(ProcessInfo))==(x64?24:16);
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
    static bool LpacInfo(IntPtr token) {
        IntPtr value=Marshal.AllocHGlobal(4);
        try {
            uint winLength,nativeLength;
            Marshal.WriteInt32(value,0x5A5A5A5A);
            bool winOk=GetTokenInformation(token,46,value,4,out winLength);
            uint winError=unchecked((uint)Marshal.GetLastWin32Error());
            bool winShape=winOk && winLength==4 && (Marshal.ReadInt32(value)==0 || Marshal.ReadInt32(value)==1);
            bool winLpac=winShape && Marshal.ReadInt32(value)!=0;
            Marshal.WriteInt32(value,0x5A5A5A5A);
            uint ntStatus=NtQueryInformationToken(token,46,value,4,out nativeLength);
            bool ntOk=ntStatus==0;
            bool ntShape=ntOk && nativeLength==4 && (Marshal.ReadInt32(value)==0 || Marshal.ReadInt32(value)==1);
            bool ntLpac=ntShape && Marshal.ReadInt32(value)!=0;
            uint nativeError=ntOk?0:RtlNtStatusToDosError(ntStatus);
            Record("{\"event\":\"lpac_fixed_query\",\"win32_success\":"+B(winOk)+",\"win32_status\":"+(winOk?0:winError)+",\"win32_shape_valid\":"+B(winShape)+",\"win32_lpac\":"+(winShape?B(winLpac):"null")+",\"native_success\":"+B(ntOk)+",\"native_win32_status\":"+nativeError+",\"native_shape_valid\":"+B(ntShape)+",\"native_lpac\":"+(ntShape?B(ntLpac):"null")+"}");
            if((winOk && !winShape) || (ntOk && !ntShape)) throw new ProbeFailure(13);
            if(winShape && ntShape && winLpac!=ntLpac) throw new ProbeFailure(13);
            if(!winShape && !ntShape) throw new ProbeFailure(winError!=0?winError:nativeError);
            return winShape?winLpac:ntLpac;
        } finally { Marshal.FreeHGlobal(value); }
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
    static bool ReadRow(string label,string path,IntPtr impersonation,bool expected) {
        IntPtr owner,group,dacl,sacl,sd; uint granted=0; bool accessAllowed=false;
        Status(GetNamedSecurityInfoW(path,1,7,out owner,out group,out dacl,out sacl,out sd));
        IntPtr privileges=Marshal.AllocHGlobal(4096);
        try {
            Mapping map=new Mapping { Read=0x120089,Write=0x120116,Execute=0x1200A0,All=FILE_ALL_ACCESS };
            uint length=4096;
            Need(AccessCheck(sd,impersonation,FILE_READ_DATA,ref map,privileges,ref length,out granted,out accessAllowed));
        } finally { Marshal.FreeHGlobal(privileges); LocalFree(sd); }
        bool opened=false; uint error=0;
        Need(ImpersonateLoggedOnUser(impersonation));
        try {
            IntPtr f=CreateFileW(path,FILE_READ_DATA,7,IntPtr.Zero,3,0x80,IntPtr.Zero);
            if(f==Invalid) error=unchecked((uint)Marshal.GetLastWin32Error());
            else { opened=true; CloseHandle(f); }
        } finally {
            // Never keep executing the managed host after failed reversion.
            if(!RevertToSelf()) { Environment.FailFast("probe impersonation reversion failed"); }
        }
        bool pass=opened==expected && accessAllowed==expected && (opened || error==5);
        Record("{\"event\":\"read_matrix\",\"fixture\":\""+label+"\",\"expected_read\":"+B(expected)+",\"access_check\":"+B(accessAllowed)+",\"opened\":"+B(opened)+",\"win32\":"+error+",\"pass\":"+B(pass)+"}");
        return pass;
    }
    static bool EmptyJob(IntPtr job) {
        IntPtr data=Marshal.AllocHGlobal(16);
        try {
            uint returned;
            Need(QueryInformationJobObject(job,3,data,16,out returned));
            return Marshal.ReadInt32(data)==0 && Marshal.ReadInt32(data,4)==0;
        } finally { Marshal.FreeHGlobal(data); }
    }
    static int RunCase(string name,bool useRestricted,bool useLpac,bool atomicJob,bool useLocalAppData) {
        currentCase=name; stage="case_start";
        string suffix=Guid.NewGuid().ToString("N");
        string profile="crucible.token.probe.v5."+suffix;
        string root=Path.Combine(Path.GetTempPath(),"crucible-token-probe-v5-"+suffix);
        IntPtr package=IntPtr.Zero,restrictSid=IntPtr.Zero,baseToken=IntPtr.Zero,restricted=IntPtr.Zero;
        IntPtr actual=IntPtr.Zero,impersonation=IntPtr.Zero,job=IntPtr.Zero,list=IntPtr.Zero;
        IntPtr restrictEntry=IntPtr.Zero,capsMemory=IntPtr.Zero,lpacMemory=IntPtr.Zero,jobMemory=IntPtr.Zero,environment=IntPtr.Zero;
        ProcessInfo pi=new ProcessInfo(); bool profileOwned=false,rootOwned=false,listReady=false,spawned=false;
        bool cleanup=true; int result=1;
        try {
            stage="layout_check"; VerifyLayouts();
            Record("{\"event\":\"case_identity\",\"restricted_requested\":"+B(useRestricted)+",\"lpac_requested\":"+B(useLpac)+",\"atomic_job_requested\":"+B(atomicJob)+",\"control_only\":"+B(!useRestricted || !useLpac)+"}");
            stage="host_token";
            Need(OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY|TOKEN_DUPLICATE|TOKEN_ASSIGN_PRIMARY,out baseToken));
            if(IsTokenRestricted(baseToken)) throw new ProbeFailure(50);
            string user; using(WindowsIdentity identity=WindowsIdentity.GetCurrent()) { user=identity.User.Value; }
            stage="profile_create";
            int hr=CreateAppContainerProfile(profile,"Crucible token probe","Disposable suspended-token experiment",IntPtr.Zero,0,out package);
            Status(unchecked((uint)hr)); profileOwned=true;
            string p=SidText(package),s=RandomSid(); Need(ConvertStringSidToSidW(s,out restrictSid));
            stage="fixtures";
            if(Directory.Exists(root) || File.Exists(root)) throw new ProbeFailure(183);
            Directory.CreateDirectory(root); rootOwned=true;
            Acl(root,user,Grant(p)+Grant(s));
            string[] labels={"user_plus_P","user_plus_S","user_plus_ARAP","user_plus_P_plus_S","user_plus_ARAP_plus_S","null_dacl"};
            string arap="S-1-15-2-2";
            string[] grants={Grant(p),Grant(s),Grant(arap),Grant(p)+Grant(s),Grant(arap)+Grant(s),""};
            bool[] expected={false,false,false,true,true,false};
            for(int i=0;i<labels.Length;i++) {
                string path=Path.Combine(root,labels[i]); File.WriteAllText(path,"synthetic probe marker");
                if(i==5) Status(SetNamedSecurityInfoW(path,1,DACL|PROTECTED_DACL,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero));
                else Acl(path,user,grants[i]);
                IntPtr control=CreateFileW(path,FILE_READ_DATA,7,IntPtr.Zero,3,0x80,IntPtr.Zero);
                Need(control!=Invalid); CloseHandle(control);
                Record("{\"event\":\"host_control\",\"fixture\":\""+labels[i]+"\",\"opened\":true}");
            }
            stage="restrict_token";
            restrictEntry=Marshal.AllocHGlobal(Marshal.SizeOf(typeof(SidAttr)));
            Marshal.StructureToPtr(new SidAttr { Sid=restrictSid,Attributes=0 },restrictEntry,false);
            // DISABLE_MAX_PRIVILEGE only. In particular, WRITE_RESTRICTED is absent.
            Need(CreateRestrictedToken(baseToken,1,0,IntPtr.Zero,0,IntPtr.Zero,1,restrictEntry,out restricted));
            if(!OnlySid(restricted,restrictSid)) throw new ProbeFailure(13);
            stage="job_create";
            job=CreateJobObjectW(IntPtr.Zero,null); Need(job!=IntPtr.Zero);
            ExtendedLimits limits=new ExtendedLimits(); limits.Basic.Flags=0x2000|8; limits.Basic.ActiveProcesses=1;
            Need(SetInformationJobObject(job,9,ref limits,(uint)Marshal.SizeOf(typeof(ExtendedLimits))));
            stage="attributes";
            int attributeCount=(useLpac?2:0)+(atomicJob?1:0);
            UIntPtr bytes=UIntPtr.Zero; InitializeProcThreadAttributeList(IntPtr.Zero,attributeCount,0,ref bytes);
            if(bytes.ToUInt64()==0 || bytes.ToUInt64()>65536) throw new ProbeFailure(13);
            list=Marshal.AllocHGlobal((int)bytes.ToUInt64());
            Need(InitializeProcThreadAttributeList(list,attributeCount,0,ref bytes)); listReady=true;
            if(useLpac) {
                capsMemory=Marshal.AllocHGlobal(Marshal.SizeOf(typeof(SecurityCapabilities)));
                Marshal.StructureToPtr(new SecurityCapabilities { AppContainerSid=package },capsMemory,false);
                Attribute(list,0x20009,capsMemory,Marshal.SizeOf(typeof(SecurityCapabilities)));
                lpacMemory=Marshal.AllocHGlobal(4); Marshal.WriteInt32(lpacMemory,1);
                Attribute(list,0x2000F,lpacMemory,4);
            }
            if(atomicJob) {
                jobMemory=Marshal.AllocHGlobal(IntPtr.Size); Marshal.WriteIntPtr(jobMemory,job);
                Attribute(list,0x2000D,jobMemory,IntPtr.Size);
            }
            StartupInfoEx si=new StartupInfoEx(); si.Startup.cb=(uint)Marshal.SizeOf(typeof(StartupInfoEx)); si.Attributes=list;
            // No inherited handles at all: the suspended guest requires no streams.
            stage="deterministic_environment";
            System.Text.StringBuilder systemBuffer=new System.Text.StringBuilder(32768);
            uint systemLength=GetSystemDirectoryW(systemBuffer,(uint)systemBuffer.Capacity);
            Need(systemLength!=0);
            if(systemLength>=(uint)systemBuffer.Capacity) throw new ProbeFailure(122);
            string systemDirectory=systemBuffer.ToString();
            string windowsDirectory=Path.GetDirectoryName(systemDirectory);
            if(String.IsNullOrEmpty(windowsDirectory) || !Path.IsPathRooted(windowsDirectory)) throw new ProbeFailure(13);
            // Sorted baseline entries; the allocator adds the final block terminator.
            // Never read or inherit the caller's environment values.
            string environmentBlock="SystemRoot="+windowsDirectory+"\0WINDIR="+windowsDirectory+"\0";
            if(useLocalAppData) {
                stage="known_folder_local_app_data";
                Guid folder=new Guid("F1B32785-6FBA-4FCF-9D55-7B8E7F157091");
                IntPtr folderMemory=IntPtr.Zero;
                try {
                    Status(unchecked((uint)SHGetKnownFolderPath(ref folder,0,IntPtr.Zero,out folderMemory)));
                    string localDirectory=Marshal.PtrToStringUni(folderMemory);
                    if(String.IsNullOrEmpty(localDirectory) || !Path.IsPathRooted(localDirectory)) throw new ProbeFailure(13);
                    environmentBlock="LOCALAPPDATA="+localDirectory+"\0"+environmentBlock;
                } finally { if(folderMemory!=IntPtr.Zero) Marshal.FreeCoTaskMem(folderMemory); }
            }
            environment=Marshal.StringToHGlobalUni(environmentBlock);
            Record("{\"event\":\"probe_identity\",\"version\":5,\"deterministic_directory_environment\":true,\"known_folder_localappdata_included\":"+B(useLocalAppData)+"}");
            stage="create_suspended";
            string executable=Path.Combine(systemDirectory,"cmd.exe");
            System.Text.StringBuilder line=new System.Text.StringBuilder("\""+executable+"\" /d /c exit 0");
            // Capture the raw BOOL and the P/Invoke cached error with no intervening API.
            int launch=CreateProcessAsUserW(useRestricted?restricted:baseToken,executable,line,IntPtr.Zero,IntPtr.Zero,false,
                CREATE_SUSPENDED|EXTENDED_STARTUPINFO_PRESENT|CREATE_UNICODE_ENVIRONMENT,environment,root,ref si,out pi);
            uint launchError=unchecked((uint)Marshal.GetLastWin32Error());
            spawned=launch!=0;
            Record("{\"event\":\"launch_result\",\"returned_success\":"+B(spawned)+",\"status\":"+(spawned?0:launchError)+",\"returned_process_handle\":"+B(pi.Process!=IntPtr.Zero)+",\"returned_thread_handle\":"+B(pi.Thread!=IntPtr.Zero)+"}");
            if(!spawned) throw new ProbeFailure(launchError);
            if(!atomicJob) {
                stage="assign_suspended_to_job";
                Need(AssignProcessToJobObject(job,pi.Process));
            }
            stage="actual_child_token";
            bool inJob; Need(IsProcessInJob(pi.Process,job,out inJob));
            Need(OpenProcessToken(pi.Process,TOKEN_QUERY|TOKEN_DUPLICATE,out actual));
            bool ac=BoolInfo(actual,29),onlyS=OnlySid(actual,restrictSid);
            bool lpac=false,zeroCaps=false,packageMatches=false;
            if(ac) {
                lpac=LpacInfo(actual);
                IntPtr capInfo=IntPtr.Zero,packageInfo=IntPtr.Zero;
                try {
                    capInfo=Info(actual,30); packageInfo=Info(actual,31);
                    zeroCaps=Marshal.ReadInt32(capInfo)==0;
                    packageMatches=Marshal.ReadIntPtr(packageInfo)!=IntPtr.Zero && EqualSid(Marshal.ReadIntPtr(packageInfo),package);
                } finally {
                    if(capInfo!=IntPtr.Zero) Marshal.FreeHGlobal(capInfo);
                    if(packageInfo!=IntPtr.Zero) Marshal.FreeHGlobal(packageInfo);
                }
            }
            bool isRestricted=IsTokenRestricted(actual);
            Record("{\"event\":\"actual_token\",\"appcontainer\":"+B(ac)+",\"lpac\":"+B(lpac)+",\"restricted\":"+B(isRestricted)+",\"only_session_restricting_sid\":"+B(onlyS)+",\"package_matches\":"+B(packageMatches)+",\"zero_capabilities\":"+(ac?B(zeroCaps):"null")+",\"in_job\":"+B(inJob)+",\"guest_never_resumed\":true}");
            bool requestedToken=ac==useLpac && lpac==useLpac && inJob
                && (!useLpac || zeroCaps && packageMatches) && (useRestricted ? onlyS && isRestricted : !onlyS);
            if(!requestedToken) throw new ProbeFailure(13);
            if(useRestricted && useLpac) {
            Need(DuplicateTokenEx(actual,TOKEN_QUERY|TOKEN_IMPERSONATE,IntPtr.Zero,2,2,out impersonation));
            bool duplicateMatches=BoolInfo(impersonation,29) && LpacInfo(impersonation)
                && OnlySid(impersonation,restrictSid) && IsTokenRestricted(impersonation);
            Record("{\"event\":\"impersonation_token\",\"preserves_lpac_and_session_restriction\":"+B(duplicateMatches)+"}");
            if(!duplicateMatches) throw new ProbeFailure(13);
            stage="read_matrix";
            bool pass=true;
            // Run the NULL DACL counterexample first; still report all six rows.
            int[] order={5,0,1,2,3,4};
            foreach(int i in order) pass=ReadRow(labels[i],Path.Combine(root,labels[i]),impersonation,expected[i]) && pass;
            result=pass ? 0 : 2;
            } else {
                Record("{\"event\":\"diagnostic_control_complete\",\"sandbox_validated\":false}");
                result=0;
            }
        } catch(ProbeFailure f) { Event("probe_api_failure",f.Code); result=1; }
          catch(Exception) { Event("probe_managed_failure",0); result=1; }
        finally {
            stage="cleanup";
            if(pi.Process!=IntPtr.Zero) {
                bool stopped=TerminateJobObject(job,1); if(!stopped) Event("terminate_job_failed",unchecked((uint)Marshal.GetLastWin32Error()));
                // Independently stop the leader even if job-membership validation failed.
                TerminateProcess(pi.Process,1);
                uint waited=WaitForSingleObject(pi.Process,5000); if(waited!=0) { cleanup=false; Event("leader_wait_failed",waited); }
            }
            Close(ref impersonation); Close(ref actual); Close(ref pi.Thread); Close(ref pi.Process);
            if(spawned) {
                bool empty=false; Stopwatch deadline=Stopwatch.StartNew();
                try { while(deadline.ElapsedMilliseconds<5000) { if(EmptyJob(job)) { empty=true; break; } Thread.Sleep(25); } }
                catch(ProbeFailure f) { Event("job_query_failed",f.Code); }
                cleanup=cleanup && empty;
                Record("{\"event\":\"job_extinction\",\"empty\":"+B(empty)+"}");
            }
            Close(ref job); Close(ref restricted); Close(ref baseToken);
            if(listReady) DeleteProcThreadAttributeList(list);
            foreach(IntPtr memory in new IntPtr[]{list,restrictEntry,capsMemory,lpacMemory,jobMemory,environment}) if(memory!=IntPtr.Zero) Marshal.FreeHGlobal(memory);
            if(restrictSid!=IntPtr.Zero) LocalFree(restrictSid);
            if(package!=IntPtr.Zero) FreeSid(package);
            if(cleanup && rootOwned) { try { Directory.Delete(root,true); } catch(Exception) { cleanup=false; Event("fixture_cleanup_failed",0); } }
            if(cleanup && profileOwned) { int deleted=DeleteAppContainerProfile(profile); if(deleted!=0) { cleanup=false; Event("profile_cleanup_failed",unchecked((uint)deleted)); } }
            Record("{\"event\":\"cleanup\",\"complete\":"+B(cleanup)+"}");
        }
        if(!cleanup) result=3;
        Record("{\"event\":\"probe_result\",\"exit_code\":"+result+",\"guest_execution_tested\":false}");
        return result;
    }
    public static int Run() {
        string[] names={"restricted_baseline","lpac_baseline","lpac_localappdata","combined_localappdata"};
        bool[] restrictions={true,false,false,true};
        bool[] lpacs={false,true,true,true};
        bool[] localAppData={false,false,true,true};
        int overall=0;
        for(int i=0;i<names.Length;i++) {
            int result=RunCase(names[i],restrictions[i],lpacs[i],true,localAppData[i]);
            if(result>overall) overall=result;
            // A failed extinction/cleanup receipt ends the whole experiment.
            if(result==3) break;
        }
        currentCase="summary";
        Record("{\"event\":\"diagnostic_result\",\"exit_code\":"+overall+",\"guest_execution_tested\":false}");
        return overall;
    }
}
'@
try { Add-Type -TypeDefinition $source -Language CSharp -ErrorAction Stop }
catch { Write-Output '{"event":"probe_compile_failure","status":0}'; exit 90 }
exit ([CrucibleWindowsTokenProbeV5]::Run())
