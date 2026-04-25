import * as React from "react";
import Svg, { Rect } from "react-native-svg";

export function QRCode(props: { value: string; size?: number; color?: string; backgroundColor?: string }) {
  const size = props.size ?? 128;
  const modules = stableMatrix(props.value, 21);
  const cell = size / modules.length;
  return (
    <Svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
      <Rect width={size} height={size} fill={props.backgroundColor ?? "#fff"} />
      {modules.map((row, y) =>
        row.map((on, x) =>
          on ? (
            <Rect
              key={`${x}-${y}`}
              x={x * cell}
              y={y * cell}
              width={Math.ceil(cell)}
              height={Math.ceil(cell)}
              fill={props.color ?? "#111"}
            />
          ) : null,
        ),
      )}
    </Svg>
  );
}

function stableMatrix(value: string, size: number) {
  let seed = 2166136261;
  for (let i = 0; i < value.length; i += 1) {
    seed ^= value.charCodeAt(i);
    seed = Math.imul(seed, 16777619);
  }
  const matrix = Array.from({ length: size }, () => Array.from({ length: size }, () => false));
  placeFinder(matrix, 0, 0);
  placeFinder(matrix, size - 7, 0);
  placeFinder(matrix, 0, size - 7);
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      if (matrix[y][x]) continue;
      seed = Math.imul(seed ^ (x * 31 + y * 131), 1103515245) + 12345;
      matrix[y][x] = ((seed >>> 16) & 1) === 1;
    }
  }
  return matrix;
}

function placeFinder(matrix: boolean[][], x0: number, y0: number) {
  for (let y = 0; y < 7; y += 1) {
    for (let x = 0; x < 7; x += 1) {
      matrix[y0 + y][x0 + x] = x === 0 || y === 0 || x === 6 || y === 6 || (x >= 2 && x <= 4 && y >= 2 && y <= 4);
    }
  }
}
