//! Windows Job Object 薄封装：让一次 spawn 的子进程及其全部后裔能被
//! 整体收尾。只依赖 std；Win32 函数手动声明（kernel32 始终已链接），
//! 与 `utils::process` 手动声明 setsid/kill 同一风格。
//!
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`：最后一个 job 句柄关闭时内核
//! 终止 job 内所有进程——覆盖三种收尾：
//! - 显式 [`JobObject::terminate`]（超时/取消强杀）；
//! - 句柄随 `ProcessTree` drop 关闭（命令正常结束后回收残留后裔，
//!   如 `start /b` 起的进程）；
//! - daemon 自身退出：句柄被系统收走，同样触发，不留孤儿树。

use std::io;
use std::ptr;

/// Win32 HANDLE。裸指针默认 !Send/!Sync，job 句柄的所有权归我们，
/// 仅在本类型内使用，显式标注可跨线程。
type Handle = *mut core::ffi::c_void;
/// Win32 BOOL。
type Bool = i32;

const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
/// AssignProcessToJobObject 要求的最小访问权限（MSDN）。
const PROCESS_SET_QUOTA: u32 = 0x0100;
const PROCESS_TERMINATE: u32 = 0x0001;

#[repr(C)]
#[derive(Default)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[repr(C)]
#[derive(Default)]
struct JobObjectExtendedLimitInformation {
    basic_limit_information: JobObjectBasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

// 布局守护：64 位下结构尺寸必须与 Win32 头文件一致（全零初始化 +
// 只置 limit_flags，布局错了会让内核读到错误的 flags）。
#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<JobObjectExtendedLimitInformation>() == 144,
    "JOBOBJECT_EXTENDED_LIMIT_INFORMATION layout drift"
);

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attrs: *mut core::ffi::c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        info_class: i32,
        info: *const core::ffi::c_void,
        len: u32,
    ) -> Bool;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
    fn TerminateJobObject(job: Handle, exit_code: u32) -> Bool;
    fn OpenProcess(access: u32, inherit: Bool, pid: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> Bool;
}

/// 打开的进程句柄（RAII）。tokio 的 `Child` 在 Windows 上不暴露进程
/// 句柄（`AsRawHandle` 只实现于 stdio 句柄），按 pid 自行打开。
pub struct ProcessHandle(Handle);

impl ProcessHandle {
    /// 以 AssignProcessToJobObject 所需的最小权限打开进程。
    /// 子进程在 spawn 与本调用之间已退出时打开失败——调用方按降级处理。
    pub fn open_for_job_assign(pid: u32) -> io::Result<Self> {
        // SAFETY: 参数平凡；返回句柄由本类型 RAII 管理。
        let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    /// 裸句柄（有效期随 `self`）。
    pub fn raw(&self) -> Handle {
        self.0
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: 句柄有效且仅在此关闭一次。
        unsafe { CloseHandle(self.0) };
    }
}

/// 一个配置了 KILL_ON_JOB_CLOSE 的 job 句柄。
pub struct JobObject(Handle);

// SAFETY: job 句柄是内核对象句柄，Win32 API 本身线程安全；所有权独占。
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    /// 创建 job 并配置 KILL_ON_JOB_CLOSE。
    pub fn new_kill_on_close() -> io::Result<Self> {
        // SAFETY: 全参数为 null，返回句柄由下方所有权管理。
        let job = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info = JobObjectExtendedLimitInformation::default();
        info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: info 指向有效的同型结构，长度字段来自 size_of。
        let ok = unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&info as *const JobObjectExtendedLimitInformation).cast(),
                size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            // SAFETY: job 有效且此后不再使用。
            unsafe { CloseHandle(job) };
            return Err(err);
        }
        Ok(Self(job))
    }

    /// 把子进程句柄（PROCESS_ALL_ACCESS / ASSIGN 权限）挂进 job。
    ///
    /// SAFETY: `process` 必须是有效的进程句柄。Win8+ 允许嵌套 job；
    /// Win7 下进程已属其他 job 时本调用失败（调用方应降级处理）。
    pub unsafe fn assign_raw(&self, process: Handle) -> io::Result<()> {
        // SAFETY: 由调用方保证 process 有效；job 自持有效。
        if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// 终止 job 内全部进程。
    pub fn terminate(&self, exit_code: u32) -> io::Result<()> {
        // SAFETY: job 自持有有效句柄。
        if unsafe { TerminateJobObject(self.0, exit_code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // SAFETY: 句柄有效且仅在此关闭一次。KILL_ON_JOB_CLOSE 语义：
        // 这是最后一个句柄时 job 内残余进程被内核终止。
        unsafe { CloseHandle(self.0) };
    }
}
