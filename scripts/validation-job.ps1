Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class PilotWeaveValidationJob {
 [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr a, string n);
 [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr h, int c, IntPtr p, uint l);
 [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr h, IntPtr p);
 [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
 public static IntPtr Attach(IntPtr process) {
   IntPtr h = CreateJobObject(IntPtr.Zero, null);
   IntPtr info = Marshal.AllocHGlobal(144);
   try {
     for(int i=0;i<144;i++) Marshal.WriteByte(info,i,0);
     Marshal.WriteInt32(info,16,0x2000);
     if(h==IntPtr.Zero || !SetInformationJobObject(h,9,info,144) || !AssignProcessToJobObject(h,process)) {
       var e=new Win32Exception(); if(h!=IntPtr.Zero) CloseHandle(h); throw e;
     }
     return h;
   } finally { Marshal.FreeHGlobal(info); }
 }
}
'@
