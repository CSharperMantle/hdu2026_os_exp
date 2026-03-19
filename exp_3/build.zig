const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const isocline = b.dependency("isocline", .{
        .target = target,
        .optimize = optimize,
    });

    const myshell_exe = b.addExecutable(.{
        .name = "myshell",
        .root_module = b.createModule(.{
            .root_source_file = b.path("myshell/main.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    myshell_exe.root_module.addImport("isocline", isocline.module("isocline"));
    myshell_exe.linkLibC();

    b.installArtifact(myshell_exe);

    const run_myshell_exe = b.addRunArtifact(myshell_exe);

    const run_step = b.step("run-myshell", "Run myshell");
    run_step.dependOn(&run_myshell_exe.step);

    const mychat_exe = b.addExecutable(.{
        .name = "mychat",
        .root_module = b.createModule(.{
            .root_source_file = b.path("mychat/main.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    mychat_exe.linkLibC();

    b.installArtifact(mychat_exe);

    const run_mychat_exe = b.addRunArtifact(mychat_exe);

    const run_mychat_step = b.step("run-mychat", "Run mychat");
    run_mychat_step.dependOn(&run_mychat_exe.step);
}
