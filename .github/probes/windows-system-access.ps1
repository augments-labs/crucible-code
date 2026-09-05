# Disposable handle-acquisition diagnostic. No object security is changed.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;

public static class RuntimeSystemProbe {
    const uint Query=8, Duplicate=2, Impersonate=4, Adjust=32;
    const int TokenUser=1, TokenPrivileges=3, TokenLevel=9;
    static readonly Stopwatch Clock=Stopwatch.StartNew();
    static int Opens;
    sealed class H : IDisposable {
        public IntPtr Value;
        public H(IntPtr value) { if(value==IntPtr.Zero) throw new Exception("null handle"); Value=value; }
        public void Dispose() {
            IntPtr value=Value; Value=IntPtr.Zero;
            if(value!=IntPtr.Zero) Check(CloseHandle(value),"close_handle");
        }
    }
    sealed class Memory : IDisposable {
        public IntPtr Value;
        public Memory(int bytes) { if(bytes<=0||bytes>65536) throw new Exception("allocation bound"); Value=Marshal.AllocHGlobal(bytes); }
        public void Dispose() { if(Value!=IntPtr.Zero) { Marshal.FreeHGlobal(Value); Value=IntPtr.Zero; } }
    }
    [StructLayout(LayoutKind.Sequential)] struct Luid { public uint Low; public int High; }
    [StructLayout(LayoutKind.Sequential)] struct Privilege { public uint Count; public Luid Id; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)] struct NativeName { public ushort Length,MaximumLength; public IntPtr Buffer; }
    [StructLayout(LayoutKind.Sequential)] struct Attributes {
        public uint Length; public IntPtr Root,Name; public uint Flags; public IntPtr Security,Quality;
    }
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool CloseHandle(IntPtr value);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentThread();
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateProcess(IntPtr process,uint status);
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,int pid);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr process,uint flags,StringBuilder name,ref uint length);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenProcessToken(IntPtr process,uint access,out IntPtr token);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenThreadToken(IntPtr thread,uint access,bool asSelf,out IntPtr token);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool GetTokenInformation(IntPtr token,int kind,IntPtr data,uint size,out uint needed);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool DuplicateTokenEx(IntPtr token,uint access,IntPtr attributes,int level,int type,out IntPtr duplicate);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool SetThreadToken(IntPtr thread,IntPtr token);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool RevertToSelf();
    [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool LookupPrivilegeValue(string system,string name,out Luid id);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool AdjustTokenPrivileges(IntPtr token,bool all,IntPtr state,uint size,IntPtr previous,IntPtr returned);
    [DllImport("ntdll.dll",ExactSpelling=true)] static extern uint NtOpenDirectoryObject(out IntPtr handle,uint access,ref Attributes attributes);
    [DllImport("ntdll.dll",ExactSpelling=true)] static extern uint NtOpenSection(out IntPtr handle,uint access,ref Attributes attributes);

    static void Bound() { if(Clock.Elapsed.TotalSeconds>150) throw new Exception("time bound"); }
    static void Check(bool ok,string stage) {
        if(!ok) {
            int error=Marshal.GetLastWin32Error();
            Console.WriteLine("NATIVE-ERROR stage="+stage+" win32="+error);
            throw new Exception("native diagnostic failure");
        }
    }
    static H ProcessToken(IntPtr process,uint access) {
        IntPtr value; Check(OpenProcessToken(process,access,out value),"open_process_token"); return new H(value);
    }
    static H Copy(H source) {
        IntPtr value; Check(DuplicateTokenEx(source.Value,Query|Impersonate|Adjust,IntPtr.Zero,2,2,out value),"duplicate_token"); return new H(value);
    }
    static Memory Info(H token,int kind,out uint size) {
        Memory memory=new Memory(65536);
        try { Check(GetTokenInformation(token.Value,kind,memory.Value,65536,out size),"token_info"); if(size>65536) throw new Exception("token bound"); return memory; }
        catch { memory.Dispose(); throw; }
    }
    static SecurityIdentifier Sid(H token) {
        uint size; using(Memory m=Info(token,TokenUser,out size)) {
            if(size<(uint)(IntPtr.Size+4)) throw new Exception("user size");
            return new SecurityIdentifier(Marshal.ReadIntPtr(m.Value));
        }
    }
    static SortedDictionary<long,uint> Privileges(H token) {
        uint size; using(Memory m=Info(token,TokenPrivileges,out size)) {
            if(size<4) throw new Exception("privilege size");
            int count=Marshal.ReadInt32(m.Value); if(count<0||count>128||4+count*12>size) throw new Exception("privilege bound");
            var result=new SortedDictionary<long,uint>();
            for(int i=0;i<count;i++) result.Add(Marshal.ReadInt64(m.Value,4+12*i),unchecked((uint)Marshal.ReadInt32(m.Value,12+12*i)));
            return result;
        }
    }
    static void EqualPrivileges(SortedDictionary<long,uint> before,SortedDictionary<long,uint> after) {
        if(before.Count!=after.Count) throw new Exception("privilege count changed");
        foreach(var item in before) { uint value; if(!after.TryGetValue(item.Key,out value)||value!=item.Value) throw new Exception("privilege attributes changed"); }
    }
    static Luid Id(string name) { Luid id; Check(LookupPrivilegeValue(null,name,out id),"lookup_privilege"); return id; }
    static long Key(Luid id) { return ((long)id.High<<32)|id.Low; }
    static void SetPrivilege(H token,Luid id,uint attributes) {
        using(Memory memory=new Memory(Marshal.SizeOf(typeof(Privilege)))) {
            Marshal.StructureToPtr(new Privilege{Count=1,Id=id,Attributes=attributes},memory.Value,false);
            bool ok=AdjustTokenPrivileges(token.Value,false,memory.Value,0,IntPtr.Zero,IntPtr.Zero);
            int error=Marshal.GetLastWin32Error();
            if(!ok||error!=0) { Console.WriteLine("NATIVE-ERROR stage=adjust_privilege win32="+error); throw new Exception("privilege adjustment"); }
        }
    }
    static void DisableAll(H token) {
        Check(AdjustTokenPrivileges(token.Value,true,IntPtr.Zero,0,IntPtr.Zero,IntPtr.Zero),"disable_all");
        foreach(var item in Privileges(token)) if((item.Value&2)!=0) throw new Exception("enabled privilege remained");
    }
    static void NoThreadToken() {
        IntPtr value;
        if(OpenThreadToken(GetCurrentThread(),Query,true,out value)) {
            using(var h=new H(value)) { throw new Exception("unexpected thread token"); }
        }
        if(Marshal.GetLastWin32Error()!=1008) throw new Exception("thread token query failed");
    }
    static void Identity(SecurityIdentifier expected,bool system) {
        IntPtr value; Check(OpenThreadToken(GetCurrentThread(),Query,true,out value),"effective_token");
        using(H token=new H(value)) {
            SecurityIdentifier sid=Sid(token);
            if(!sid.Equals(expected)||sid.IsWellKnown(WellKnownSidType.LocalSystemSid)!=system) throw new Exception("effective identity mismatch");
            uint size; using(Memory m=Info(token,TokenLevel,out size)) if(size!=4||Marshal.ReadInt32(m.Value)!=2) throw new Exception("impersonation level mismatch");
        }
    }
    static void Observe(int index,string name,uint access,string cell,bool system) {
        Bound(); if(++Opens>27) throw new Exception("open bound");
        using(Memory text=new Memory((name.Length+1)*2))
        using(Memory native=new Memory(Marshal.SizeOf(typeof(NativeName)))) {
            Marshal.Copy((name+'\0').ToCharArray(),0,text.Value,name.Length+1);
            Marshal.StructureToPtr(new NativeName{Length=(ushort)(name.Length*2),MaximumLength=(ushort)((name.Length+1)*2),Buffer=text.Value},native.Value,false);
            Attributes attributes=new Attributes{Length=(uint)Marshal.SizeOf(typeof(Attributes)),Name=native.Value,Flags=0x40};
            IntPtr raw; uint status=index==0?NtOpenDirectoryObject(out raw,access,ref attributes):NtOpenSection(out raw,access,ref attributes);
            if(status==0&&raw==IntPtr.Zero) throw new Exception("successful open returned null");
            if(raw!=IntPtr.Zero) using(H handle=new H(raw)) { }
            Console.WriteLine("RUNTIME-OPEN cell="+cell+" object="+index+" system="+(system?1:0)+" access="+access+" ntstatus="+status);
        }
    }
    static void Cell(H token,SecurityIdentifier expected,string name,bool system) {
        NoThreadToken(); Check(SetThreadToken(IntPtr.Zero,token.Value),"impersonate");
        try {
            string[] paths={@"\KnownDlls",@"\KnownDlls\ntdll.dll",@"\KnownDlls\kernel32.dll"};
            uint[] access={0x20000,0x40000,0x80000};
            for(int i=0;i<paths.Length;i++) foreach(uint mask in access) {
                Identity(expected,system); Observe(i,paths[i],mask,name,system);
            }
        } finally {
            if(!RevertToSelf()) { Console.WriteLine("RESTORATION-FAILURE"); TerminateProcess(GetCurrentProcess(),77); Environment.Exit(77); }
        }
        NoThreadToken();
    }
    static H SystemToken(H original,SortedDictionary<long,uint> before) {
        Luid debug=Id("SeDebugPrivilege"); uint old;
        if(!before.TryGetValue(Key(debug),out old)) { Console.WriteLine("SYSTEM-UNAVAILABLE debug_privilege_absent"); return null; }
        H result=null;
        try {
          try {
            SetPrivilege(original,debug,old|2);
            Process[] sources=Process.GetProcessesByName("winlogon");
            try {
                if(sources.Length==0||sources.Length>16) throw new Exception("source count bound");
                Array.Sort(sources,(a,b)=>a.Id.CompareTo(b.Id));
                IntPtr raw=OpenProcess(0x1000,false,sources[0].Id); Check(raw!=IntPtr.Zero,"open_fixed_source");
                using(H process=new H(raw)) {
                    StringBuilder image=new StringBuilder(512); uint length=512;
                    Check(QueryFullProcessImageName(process.Value,0,image,ref length),"source_image");
                    if(!String.Equals(image.ToString(),Path.Combine(Environment.SystemDirectory,"winlogon.exe"),StringComparison.OrdinalIgnoreCase)) throw new Exception("source image mismatch");
                    using(H source=ProcessToken(process.Value,Query|Duplicate)) {
                        if(!Sid(source).IsWellKnown(WellKnownSidType.LocalSystemSid)) throw new Exception("source SID mismatch");
                        result=Copy(source);
                        Console.WriteLine("SYSTEM-SOURCE verified=1");
                    }
                }
            } finally { foreach(Process source in sources) source.Dispose(); }
        } finally {
            SetPrivilege(original,debug,old);
            EqualPrivileges(before,Privileges(original));
          }
          return result;
        } catch { if(result!=null) result.Dispose(); throw; }
    }
    static int Execute() {
        NoThreadToken();
        using(H original=ProcessToken(GetCurrentProcess(),Query|Duplicate|Adjust)) {
            var sid=Sid(original); var before=Privileges(original);
            if(sid.IsWellKnown(WellKnownSidType.LocalSystemSid)) throw new Exception("expected admin controller");
            try {
                using(H admin=Copy(original)) { DisableAll(admin); Cell(admin,sid,"admin_no_privileges",false); }
                using(H system=SystemToken(original,before)) {
                    if(system==null) return 77;
                    var systemSid=Sid(system); DisableAll(system);
                    Cell(system,systemSid,"system_no_privileges",true);
                    Luid own=Id("SeTakeOwnershipPrivilege");
                    if(Privileges(system).ContainsKey(Key(own))) {
                        SetPrivilege(system,own,2);
                        foreach(var p in Privileges(system)) if((p.Value&2)!=0&&p.Key!=Key(own)) throw new Exception("unexpected enabled privilege");
                        Cell(system,systemSid,"system_take_ownership_only",true);
                    } else Console.WriteLine("OWNERSHIP-PRIVILEGE unavailable=1");
                }
            } finally {
                NoThreadToken(); if(!Sid(original).Equals(sid)) throw new Exception("original identity changed");
                EqualPrivileges(before,Privileges(original)); Console.WriteLine("RESTORATION-PASS");
            }
        }
        Console.WriteLine("SYSTEM-PREREQUISITE-COMPLETE opens="+Opens+" acl_changed=0 guest_started=0 full_sandbox_tested=0");
        return 0;
    }
    public static int Run() {
        try { return Execute(); }
        catch(Exception error) { Console.WriteLine("SYSTEM-PROBE-FAILED type="+error.GetType().Name+" opens="+Opens); return 77; }
    }
}
'@
exit [RuntimeSystemProbe]::Run()
