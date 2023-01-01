struct Slime {
  value: f32,
  _padding0: f32,
  _padding1: f32,
  _padding2: f32,
}

@group(0) @binding(0)
var<storage, read_write> slime: Slime;

@compute @workgroup_size(1, 1, 1)
fn update(@builtin(global_invocation_id) invocation_id: vec3<u32>) {
    slime.value = slime.value + 1.0;

    storageBarrier();
}
