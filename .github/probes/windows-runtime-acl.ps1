# Disposable, read-only feasibility probe. No object ACL is changed.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Security.Principal;

public static class RuntimeAclProbe {
    [StructLayout(LayoutKind.Sequential)] struct NativeName {
        public ushort Length, MaximumLength;
        public IntPtr Buffer;
    }
    [StructLayout(LayoutKind.Sequential)] struct Attributes {
        public uint Length;
        public IntPtr Root, Name;
        public uint Flags;
        public IntPtr Security, Quality;
    }
    [DllImport("ntdll.dll", ExactSpelling=true)]
    static extern uint NtOpenDirectoryObject(out IntPtr handle,uint access,ref Attributes attributes);
    [DllImport("ntdll.dll", ExactSpelling=true)]
    static extern uint NtOpenSection(out IntPtr handle,uint access,ref Attributes attributes);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr memory);
    [DllImport("advapi32.dll", SetLastError=true)]
    static extern uint GetSecurityInfo(IntPtr handle,int kind,uint information,out IntPtr owner,
        out IntPtr group,out IntPtr dacl,out IntPtr sacl,out IntPtr descriptor);
    [DllImport("advapi32.dll")]
    static extern bool IsWellKnownSid(IntPtr sid,int kind);

    static void Observe(string name,int index,bool directory,bool administration) {
        IntPtr text=IntPtr.Zero,native=IntPtr.Zero,handle=IntPtr.Zero,descriptor=IntPtr.Zero;
        try {
            if(name.Length>128) throw new InvalidOperationException("name bound");
            text=Marshal.StringToHGlobalUni(name);
            native=Marshal.AllocHGlobal(Marshal.SizeOf(typeof(NativeName)));
            Marshal.StructureToPtr(new NativeName {
                Length=(ushort)(name.Length*2), MaximumLength=(ushort)(name.Length*2+2),Buffer=text
            },native,false);
            Attributes attributes=new Attributes {
                Length=(uint)Marshal.SizeOf(typeof(Attributes)),Name=native,Flags=0x40
            };
            uint access=administration?0x60000U:(0x20000U|(directory?3U:12U));
            uint status=directory?NtOpenDirectoryObject(out handle,access,ref attributes)
                :NtOpenSection(out handle,access,ref attributes);
            Console.WriteLine("{\"event\":\"runtime_acl_open\",\"object\":"+index+
                ",\"administration_requested\":"+(administration?"true":"false")+
                ",\"access\":"+access+",\"ntstatus\":"+status+"}");
            if(status==0&&!administration) {
                IntPtr owner,group,dacl,sacl;
                uint query=GetSecurityInfo(handle,6,5,out owner,out group,out dacl,out sacl,out descriptor);
                bool system=query==0&&owner!=IntPtr.Zero&&IsWellKnownSid(owner,22);
                Console.WriteLine("{\"event\":\"runtime_acl_metadata\",\"object\":"+index+
                    ",\"win32\":"+query+",\"owner_is_system\":"+(system?"true":"false")+
                    ",\"non_null_dacl\":"+(query==0&&dacl!=IntPtr.Zero?"true":"false")+"}");
            }
        } finally {
            if(descriptor!=IntPtr.Zero) LocalFree(descriptor);
            if(handle!=IntPtr.Zero&&!CloseHandle(handle)) throw new InvalidOperationException("handle close");
            if(native!=IntPtr.Zero) Marshal.FreeHGlobal(native);
            if(text!=IntPtr.Zero) Marshal.FreeHGlobal(text);
        }
    }
    public static void Run() {
        using(WindowsIdentity identity=WindowsIdentity.GetCurrent()) {
            bool admin=new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
            Console.WriteLine("{\"event\":\"host_identity\",\"elevated_administrator\":"+
                (admin?"true":"false")+",\"os_version\":\""+Environment.OSVersion.Version+"\"}");
        }
        string[] names={@"\KnownDlls",@"\KnownDlls\ntdll.dll",@"\KnownDlls\kernel32.dll"};
        for(int i=0;i<names.Length;i++) {
            Observe(names[i],i,i==0,false);
            Observe(names[i],i,i==0,true);
        }
        Console.WriteLine("{\"event\":\"probe_complete\",\"acl_changed\":false,\"guest_started\":false,\"sandbox_validated\":false}");
    }
}
'@
[RuntimeAclProbe]::Run()
