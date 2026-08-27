// 方形节点的四层渲染 program —— 与圆形 createNodeBorderProgram 完全同构：
// 状态环(0.1) → 钢灰描边(0.07) → 深色壳(0.3) → 微彩核心(fill)。
// 圆形用欧氏距离 length()，方形改用切比雪夫距离 max(|x|,|y|)，
// 几何沿用 @sigma/node-square：6 顶点四边形、角点 ±45°/±135°、sqrt8 缩放
//（等效半边长与圆形半径一致，hover/selected 状态环两种形状表现对齐）。

import { NodeProgram } from "sigma/rendering";
import type { ProgramInfo } from "sigma/rendering";
import type { NodeDisplayData, RenderParams } from "sigma/types";
import { floatColor } from "sigma/utils";

const { UNSIGNED_BYTE, FLOAT } = WebGLRenderingContext;

const UNIFORMS = ["u_sizeRatio", "u_correctionRatio", "u_cameraAngle", "u_matrix"] as const;

const VERTEX_SHADER_SOURCE = /*glsl*/ `
attribute vec2 a_position;
attribute float a_size;
attribute float a_angle;

uniform mat3 u_matrix;
uniform float u_sizeRatio;
uniform float u_cameraAngle;
uniform float u_correctionRatio;

varying vec2 v_diffVector;
varying float v_radius;

#ifdef PICKING_MODE
attribute vec4 a_id;
varying vec4 v_color;
#else
attribute vec4 a_ringColor;
attribute vec4 a_borderColor;
attribute vec4 a_shellColor;
attribute vec4 a_color;
varying vec4 v_ringColor;
varying vec4 v_borderColor;
varying vec4 v_shellColor;
varying vec4 v_fillColor;
#endif

const float sqrt_8 = sqrt(8.0);
const float sqrt_2 = sqrt(2.0);

void main() {
  float size = a_size * u_correctionRatio / u_sizeRatio * sqrt_8;
  // 屏幕上保持轴对齐（跟随相机旋转），切比雪夫度量用未旋转的本地坐标
  float angle = a_angle + u_cameraAngle;
  vec2 diffVector = size * vec2(cos(angle), sin(angle));
  vec2 position = a_position + diffVector;
  gl_Position = vec4((u_matrix * vec3(position, 1)).xy, 0, 1);

  v_diffVector = size * vec2(cos(a_angle), sin(a_angle));
  v_radius = size / sqrt_2; // 半边长，与圆形程序的半径等值

  #ifdef PICKING_MODE
  v_color = a_id;
  #else
  v_ringColor = a_ringColor;
  v_borderColor = a_borderColor;
  v_shellColor = a_shellColor;
  v_fillColor = a_color;
  #endif
}
`;

const FRAGMENT_SHADER_SOURCE = /*glsl*/ `
precision highp float;

varying vec2 v_diffVector;
varying float v_radius;

#ifdef PICKING_MODE
varying vec4 v_color;
#else
varying vec4 v_ringColor;
varying vec4 v_borderColor;
varying vec4 v_shellColor;
varying vec4 v_fillColor;
#endif

uniform float u_correctionRatio;

const float bias = 255.0 / 254.0;
const vec4 transparent = vec4(0.0, 0.0, 0.0, 0.0);

void main(void) {
  float dist = max(abs(v_diffVector.x), abs(v_diffVector.y));

  #ifdef PICKING_MODE
  if (dist > v_radius)
    gl_FragColor = transparent;
  else {
    gl_FragColor = v_color;
    gl_FragColor.a *= bias;
  }
  #else
  float aaBorder = 2.0 * u_correctionRatio;
  // 层厚比例与圆形配置一致：0.1 / 0.07 / 0.3 / 剩余为核心
  float r0 = v_radius;
  float r1 = r0 - v_radius * 0.1;
  float r2 = r1 - v_radius * 0.07;
  float r3 = r2 - v_radius * 0.3;

  vec4 c1 = v_ringColor;   c1.a *= bias;
  vec4 c2 = v_borderColor; c2.a *= bias;
  vec4 c3 = v_shellColor;  c3.a *= bias;
  vec4 c4 = v_fillColor;   c4.a *= bias;

  if (dist > r0) {
    gl_FragColor = transparent;
  } else if (dist > r0 - aaBorder) {
    gl_FragColor = mix(c1, transparent, (dist - r0 + aaBorder) / aaBorder);
  } else if (dist > r1) {
    gl_FragColor = c1;
  } else if (dist > r1 - aaBorder) {
    gl_FragColor = mix(c2, c1, (dist - r1 + aaBorder) / aaBorder);
  } else if (dist > r2) {
    gl_FragColor = c2;
  } else if (dist > r2 - aaBorder) {
    gl_FragColor = mix(c3, c2, (dist - r2 + aaBorder) / aaBorder);
  } else if (dist > r3) {
    gl_FragColor = c3;
  } else if (dist > r3 - aaBorder) {
    gl_FragColor = mix(c4, c3, (dist - r3 + aaBorder) / aaBorder);
  } else {
    gl_FragColor = c4;
  }
  #endif
}
`;

export class NodeSquareShellProgram extends NodeProgram<(typeof UNIFORMS)[number]> {
  getDefinition() {
    return {
      VERTICES: 6,
      VERTEX_SHADER_SOURCE,
      FRAGMENT_SHADER_SOURCE,
      METHOD: WebGLRenderingContext.TRIANGLES,
      UNIFORMS,
      ATTRIBUTES: [
        { name: "a_position", size: 2, type: FLOAT },
        { name: "a_id", size: 4, type: UNSIGNED_BYTE, normalized: true },
        { name: "a_size", size: 1, type: FLOAT },
        { name: "a_ringColor", size: 4, type: UNSIGNED_BYTE, normalized: true },
        { name: "a_borderColor", size: 4, type: UNSIGNED_BYTE, normalized: true },
        { name: "a_shellColor", size: 4, type: UNSIGNED_BYTE, normalized: true },
        { name: "a_color", size: 4, type: UNSIGNED_BYTE, normalized: true },
      ],
      CONSTANT_ATTRIBUTES: [{ name: "a_angle", size: 1, type: FLOAT }],
      CONSTANT_DATA: [
        [Math.PI / 4],
        [(3 * Math.PI) / 4],
        [-Math.PI / 4],
        [(3 * Math.PI) / 4],
        [-Math.PI / 4],
        [(-3 * Math.PI) / 4],
      ],
    };
  }

  processVisibleItem(nodeIndex: number, startIndex: number, data: NodeDisplayData): void {
    const array = this.array;
    const d = data as NodeDisplayData & {
      ringColor?: string;
      borderColor?: string;
      shellColor?: string;
    };
    array[startIndex++] = data.x;
    array[startIndex++] = data.y;
    array[startIndex++] = nodeIndex;
    array[startIndex++] = data.size;
    array[startIndex++] = floatColor(d.ringColor || "rgba(0,0,0,0)");
    array[startIndex++] = floatColor(d.borderColor || "rgba(0,0,0,0)");
    array[startIndex++] = floatColor(d.shellColor || "rgba(0,0,0,0)");
    array[startIndex++] = floatColor(data.color);
  }

  setUniforms(params: RenderParams, { gl, uniformLocations }: ProgramInfo): void {
    const { u_sizeRatio, u_correctionRatio, u_cameraAngle, u_matrix } = uniformLocations;
    gl.uniform1f(u_sizeRatio, params.sizeRatio);
    gl.uniform1f(u_correctionRatio, params.correctionRatio);
    gl.uniform1f(u_cameraAngle, params.cameraAngle);
    gl.uniformMatrix3fv(u_matrix, false, params.matrix);
  }
}
