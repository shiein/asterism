export type AnnotationKind =
  | "rectangle"
  | "ellipse"
  | "arrow"
  | "line"
  | "brush"
  | "text"
  | "mosaic"
  | "blur";

/** 图片逻辑坐标，不是 CSS 坐标。 */
export interface Annotation {
  id: string;
  kind: AnnotationKind;
  geometry: number[];
  style: Record<string, unknown>;
  zIndex: number;
}

export interface AnnotationScene {
  items: Annotation[];
}
