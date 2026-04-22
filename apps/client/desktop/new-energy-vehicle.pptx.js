const pptxgen = require("pptxgenjs");

// 创建 PPT 实例
const pptx = new pptxgen();

// 设置 PPT 基本信息
pptx.title = "新能源电动车发展";
pptx.author = "Cteno";
pptx.company = "智能分析";

// 定义主题颜色 - Ocean Gradient
const colors = {
  primary: "065A82",      // 深蓝
  secondary: "1C7293",    // 蓝绿色
  accent: "21295C",       // 午夜蓝
  highlight: "02C39A",    // 薄荷绿
  text: "334155",         // 深灰色
  lightText: "94a3b8",    // 浅灰色
  lightBg: "f1f5f9",      // 浅灰背景
  white: "ffffff",
  gradient1: "065A82",    // 渐变色1
  gradient2: "1C7293"     // 渐变色2
};

// ========== 第1页：封面 ==========
const slide1 = pptx.addSlide();
slide1.background = { color: colors.primary };

slide1.addText("新能源电动车发展", {
  x: 0.5, y: 1.8, w: 9, h: 1,
  fontSize: 44, bold: true, color: colors.white,
  fontFace: "Microsoft YaHei"
});

slide1.addText("技术演进、市场现状与未来趋势", {
  x: 0.5, y: 2.9, w: 9, h: 0.6,
  fontSize: 22, color: colors.highlight,
  fontFace: "Microsoft YaHei"
});

slide1.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 3.6, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

slide1.addText("从替代能源到智能出行\n电动化、智能化、网联化、共享化", {
  x: 0.5, y: 3.9, w: 9, h: 0.9,
  fontSize: 16, color: colors.lightText,
  fontFace: "Microsoft YaHei"
});

slide1.addText("迈向可持续交通的未来", {
  x: 0.5, y: 4.9, w: 9, h: 0.5,
  fontSize: 14, italic: true, color: colors.highlight,
  fontFace: "Microsoft YaHei"
});

// ========== 第2页：目录 ==========
const slide2 = pptx.addSlide();
slide2.background = { color: colors.white };

slide2.addText("目录", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 32, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide2.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

const tocItems = [
  { title: "引言：新能源电动车定义与背景", page: 3 },
  { title: "发展历程：从早期到现代", page: 4 },
  { title: "关键技术：三电系统与智能化", page: 5 },
  { title: "市场现状：全球与中国数据", page: 6 },
  { title: "政策环境：各国支持政策", page: 7 },
  { title: "挑战与瓶颈", page: 8 },
  { title: "未来趋势：自动驾驶与能源互联网", page: 9 },
  { title: "结论", page: 10 }
];

tocItems.forEach((item, index) => {
  const y = 1.3 + index * 0.6;
  
  // 项目符号
  slide2.addShape(pptx.shapes.OVAL, {
    x: 0.7, y: y + 0.15, w: 0.08, h: 0.08,
    fill: { color: colors.secondary }
  });
  
  // 标题
  slide2.addText(item.title, {
    x: 0.9, y: y, w: 7, h: 0.4,
    fontSize: 14, color: colors.text,
    fontFace: "Microsoft YaHei"
  });
  
  // 页码
  slide2.addText(item.page.toString(), {
    x: 8.0, y: y, w: 1, h: 0.4,
    fontSize: 14, color: colors.secondary,
    fontFace: "Microsoft YaHei", align: "right"
  });
});

// ========== 第3页：引言：新能源电动车定义与背景 ==========
const slide3 = pptx.addSlide();
slide3.background = { color: colors.white };

slide3.addText("引言：新能源电动车定义与背景", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 30, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide3.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

// 定义
slide3.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 1.3, w: 9, h: 1.0,
  fill: { color: colors.lightBg }
});

slide3.addText("新能源电动车（NEV）", {
  x: 0.7, y: 1.4, w: 8.5, h: 0.4,
  fontSize: 18, bold: true, color: colors.secondary,
  fontFace: "Microsoft YaHei"
});

slide3.addText(
  "指采用新型能源（电力、氢能等）作为动力，替代传统燃油的汽车。主要包括纯电动车（BEV）、插电式混合动力车（PHEV）、燃料电池车（FCEV）等。",
  {
    x: 0.7, y: 1.8, w: 8.5, h: 0.5,
    fontSize: 13, color: colors.text,
    fontFace: "Microsoft YaHei"
  }
);

// 背景
slide3.addText("发展背景", {
  x: 0.5, y: 2.5, w: 9, h: 0.5,
  fontSize: 18, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

const backgroundPoints = [
  "气候变化与碳排放压力：交通领域占全球碳排放约 24%",
  "能源安全：减少对石油进口的依赖",
  "技术创新：电池成本下降，性能提升",
  "政策驱动：各国设定燃油车禁售时间表",
  "消费者需求：环保意识增强，使用成本更低"
];

backgroundPoints.forEach((point, index) => {
  slide3.addText("• " + point, {
    x: 0.7, y: 3.0 + index * 0.5, w: 8.5, h: 0.45,
    fontSize: 12, color: colors.text,
    fontFace: "Microsoft YaHei"
  });
});

// ========== 第4页：发展历程：从早期到现代 ==========
const slide4 = pptx.addSlide();
slide4.background = { color: colors.white };

slide4.addText("发展历程：从早期到现代", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 30, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide4.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

// 时间线
slide4.addShape(pptx.shapes.RECTANGLE, {
  x: 1.0, y: 2.0, w: 8.0, h: 0.03,
  fill: { color: colors.lightText }
});

const timeline = [
  { year: "1830s", title: "早期探索", desc: "第一辆电动车问世", color: colors.lightText },
  { year: "1990s", title: "现代复兴", desc: "通用 EV1，丰田普锐斯", color: colors.secondary },
  { year: "2008", title: "特斯拉崛起", desc: "Roadster 发布，Model S 引领", color: colors.accent },
  { year: "2015", title: "中国发力", desc: "政策扶持，蔚来、小鹏等新势力", color: colors.highlight },
  { year: "2020s", title: "全球普及", desc: "传统车企转型，电动化加速", color: colors.primary }
];

timeline.forEach((item, index) => {
  const x = 1.0 + index * 1.8;
  
  // 时间点圆圈
  slide4.addShape(pptx.shapes.OVAL, {
    x: x - 0.1, y: 1.85, w: 0.25, h: 0.25,
    fill: { color: item.color }
  });
  
  // 年份
  slide4.addText(item.year, {
    x: x - 0.4, y: 1.5, w: 1.0, h: 0.35,
    fontSize: 14, bold: true, color: item.color,
    fontFace: "Microsoft YaHei", align: "center"
  });
  
  // 标题
  slide4.addText(item.title, {
    x: x - 0.6, y: 2.2, w: 1.4, h: 0.4,
    fontSize: 12, bold: true, color: colors.primary,
    fontFace: "Microsoft YaHei", align: "center"
  });
  
  // 描述
  slide4.addText(item.desc, {
    x: x - 0.8, y: 2.6, w: 2.0, h: 0.8,
    fontSize: 10, color: colors.text,
    fontFace: "Microsoft YaHei", align: "center"
  });
});

// 底部总结
slide4.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 4.5, w: 9, h: 0.8,
  fill: { color: colors.lightBg }
});

slide4.addText("从概念到主流，新能源电动车经历了近两个世纪的曲折发展，如今正迎来历史性机遇", {
  x: 0.5, y: 4.5, w: 9, h: 0.8,
  fontSize: 14, color: colors.primary,
  fontFace: "Microsoft YaHei", align: "center", valign: "middle"
});

// ========== 第5页：关键技术：三电系统与智能化 ==========
const slide5 = pptx.addSlide();
slide5.background = { color: colors.white };

slide5.addText("关键技术：三电系统与智能化", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 30, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide5.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

// 三电系统
slide5.addText("三电系统", {
  x: 0.5, y: 1.3, w: 4.3, h: 0.5,
  fontSize: 18, bold: true, color: colors.secondary,
  fontFace: "Microsoft YaHei"
});

const threeElectric = [
  { title: "电池", desc: "能量密度提升\n成本持续下降\n固态电池突破", icon: "🔋" },
  { title: "电机", desc: "高效永磁同步\n集成化设计\n功率密度提升", icon: "⚡" },
  { title: "电控", desc: "智能能量管理\n热管理优化\n整车控制集成", icon: "🎛️" }
];

threeElectric.forEach((item, index) => {
  const x = 0.5 + index * 1.5;
  
  slide5.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
    x: x, y: 1.9, w: 1.3, h: 2.2,
    fill: { color: colors.lightBg }
  });
  
  slide5.addText(item.icon, {
    x: x, y: 2.0, w: 1.3, h: 0.5,
    fontSize: 24, align: "center"
  });
  
  slide5.addText(item.title, {
    x: x, y: 2.5, w: 1.3, h: 0.4,
    fontSize: 14, bold: true, color: colors.primary,
    fontFace: "Microsoft YaHei", align: "center"
  });
  
  slide5.addText(item.desc, {
    x: x + 0.1, y: 2.9, w: 1.1, h: 1.0,
    fontSize: 10, color: colors.text,
    fontFace: "Microsoft YaHei", align: "center"
  });
});

// 智能化
slide5.addText("智能化技术", {
  x: 5.0, y: 1.3, w: 4.5, h: 0.5,
  fontSize: 18, bold: true, color: colors.highlight,
  fontFace: "Microsoft YaHei"
});

const smartTech = [
  "自动驾驶：L2→L4渐进",
  "车联网：V2X通信",
  "智能座舱：多屏交互、语音助手",
  "OTA远程升级：持续迭代"
];

smartTech.forEach((item, index) => {
  slide5.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
    x: 5.0, y: 1.9 + index * 0.55, w: 4.5, h: 0.45,
    fill: { color: colors.white },
    line: { color: colors.highlight, width: 1 }
  });
  
  slide5.addText("✓ " + item, {
    x: 5.2, y: 1.9 + index * 0.55, w: 4.1, h: 0.45,
    fontSize: 12, color: colors.text,
    fontFace: "Microsoft YaHei", valign: "middle"
  });
});

// ========== 第6页：市场现状：全球与中国数据 ==========
const slide6 = pptx.addSlide();
slide6.background = { color: colors.white };

slide6.addText("市场现状：全球与中国数据", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 30, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide6.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

// 全球市场
slide6.addText("全球市场（2025年）", {
  x: 0.5, y: 1.3, w: 4.3, h: 0.5,
  fontSize: 16, bold: true, color: colors.secondary,
  fontFace: "Microsoft YaHei"
});

slide6.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 1.8, w: 4.3, h: 2.5,
  fill: { color: colors.lightBg }
});

const globalData = [
  { label: "年销量", value: "1800万辆", desc: "占新车销量 18%" },
  { label: "保有量", value: "1.2亿辆", desc: "累计渗透率 8%" },
  { label: "TOP 3品牌", value: "特斯拉、比亚迪、大众", desc: "合计占 35%" },
  { label: "区域分布", value: "中国 55%", desc: "欧洲 25%，美国 15%" }
];

globalData.forEach((item, index) => {
  const y = 2.0 + index * 0.6;
  
  slide6.addText(item.label, {
    x: 0.7, y: y, w: 1.5, h: 0.4,
    fontSize: 12, color: colors.text,
    fontFace: "Microsoft YaHei"
  });
  
  slide6.addText(item.value, {
    x: 2.2, y: y, w: 2.5, h: 0.4,
    fontSize: 14, bold: true, color: colors.primary,
    fontFace: "Microsoft YaHei"
  });
  
  slide6.addText(item.desc, {
    x: 0.7, y: y + 0.25, w: 4.0, h: 0.3,
    fontSize: 10, color: colors.lightText,
    fontFace: "Microsoft YaHei"
  });
});

// 中国市场
slide6.addText("中国市场（2025年）", {
  x: 5.0, y: 1.3, w: 4.5, h: 0.5,
  fontSize: 16, bold: true, color: colors.highlight,
  fontFace: "Microsoft YaHei"
});

slide6.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 5.0, y: 1.8, w: 4.5, h: 2.5,
  fill: { color: colors.lightBg }
});

const chinaData = [
  { label: "年销量", value: "1000万辆", desc: "渗透率超 40%" },
  { label: "保有量", value: "6000万辆", desc: "全球第一" },
  { label: "TOP 3品牌", value: "比亚迪、特斯拉、蔚来", desc: "新势力崛起" },
  { label: "出口量", value: "300万辆", desc: "成为全球工厂" }
];

chinaData.forEach((item, index) => {
  const y = 2.0 + index * 0.6;
  
  slide6.addText(item.label, {
    x: 5.2, y: y, w: 1.5, h: 0.4,
    fontSize: 12, color: colors.text,
    fontFace: "Microsoft YaHei"
  });
  
  slide6.addText(item.value, {
    x: 6.7, y: y, w: 2.5, h: 0.4,
    fontSize: 14, bold: true, color: colors.highlight,
    fontFace: "Microsoft YaHei"
  });
  
  slide6.addText(item.desc, {
    x: 5.2, y: y + 0.25, w: 4.0, h: 0.3,
    fontSize: 10, color: colors.lightText,
    fontFace: "Microsoft YaHei"
  });
});

// ========== 第7页：政策环境：各国支持政策 ==========
const slide7 = pptx.addSlide();
slide7.background = { color: colors.white };

slide7.addText("政策环境：各国支持政策", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 30, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide7.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

const policies = [
  { country: "中国", measures: ["双积分政策", "购置税减免", "充电设施补贴", "2035年停售燃油车"], color: colors.secondary },
  { country: "欧盟", measures: ["2035年禁售燃油车", "碳排放标准", "电动车补贴", "充电网络建设"], color: colors.primary },
  { country: "美国", measures: ["通胀削减法案", "税收抵免", "本土生产要求", "基础设施投资"], color: colors.accent },
  { country: "日本", measures: ["2035年电动化目标", "氢能战略", "充电桩补贴", "研发支持"], color: colors.highlight }
];

policies.forEach((policy, index) => {
  const x = 0.5 + index * 2.3;
  
  // 国家标签
  slide7.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
    x: x, y: 1.3, w: 2.1, h: 0.5,
    fill: { color: policy.color }
  });
  
  slide7.addText(policy.country, {
    x: x, y: 1.3, w: 2.1, h: 0.5,
    fontSize: 16, bold: true, color: colors.white,
    fontFace: "Microsoft YaHei", align: "center", valign: "middle"
  });
  
  // 政策列表
  slide7.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
    x: x, y: 1.8, w: 2.1, h: 2.8,
    fill: { color: colors.lightBg }
  });
  
  policy.measures.forEach((measure, mIndex) => {
    slide7.addText("• " + measure, {
      x: x + 0.1, y: 1.9 + mIndex * 0.6, w: 1.9, h: 0.5,
      fontSize: 10, color: colors.text,
      fontFace: "Microsoft YaHei"
    });
  });
});

// 底部总结
slide7.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 4.8, w: 9, h: 0.7,
  fill: { color: colors.primary }
});

slide7.addText("全球政策形成合力，加速电动化转型", {
  x: 0.5, y: 4.8, w: 9, h: 0.7,
  fontSize: 14, bold: true, color: colors.white,
  fontFace: "Microsoft YaHei", align: "center", valign: "middle"
});

// ========== 第8页：挑战与瓶颈 ==========
const slide8 = pptx.addSlide();
slide8.background = { color: colors.white };

slide8.addText("挑战与瓶颈", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 30, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide8.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

const challenges = [
  { category: "技术", items: ["电池续航焦虑", "充电速度慢", "低温性能衰减", "原材料供应紧张"] },
  { category: "基础设施", items: ["充电桩数量不足", "分布不均", "标准不统一", "电网负荷压力"] },
  { category: "成本", items: ["初期购置成本高", "保险维修费用", "电池更换成本", "残值率低"] },
  { category: "其他", items: ["安全性与可靠性", "回收利用体系", "消费者接受度", "传统产业转型"] }
];

challenges.forEach((challenge, index) => {
  const x = 0.5 + index * 2.3;
  
  // 分类标题
  slide8.addText(challenge.category, {
    x: x, y: 1.3, w: 2.1, h: 0.4,
    fontSize: 16, bold: true, color: colors.primary,
    fontFace: "Microsoft YaHei"
  });
  
  // 列表
  slide8.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
    x: x, y: 1.8, w: 2.1, h: 3.0,
    fill: { color: colors.lightBg }
  });
  
  challenge.items.forEach((item, iIndex) => {
    slide8.addText("✗ " + item, {
      x: x + 0.2, y: 1.9 + iIndex * 0.6, w: 1.8, h: 0.5,
      fontSize: 10, color: colors.text,
      fontFace: "Microsoft YaHei"
    });
  });
});

// ========== 第9页：未来趋势：自动驾驶与能源互联网 ==========
const slide9 = pptx.addSlide();
slide9.background = { color: colors.white };

slide9.addText("未来趋势：自动驾驶与能源互联网", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 28, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide9.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

// 自动驾驶
slide9.addText("自动驾驶", {
  x: 0.5, y: 1.3, w: 4.3, h: 0.5,
  fontSize: 18, bold: true, color: colors.secondary,
  fontFace: "Microsoft YaHei"
});

slide9.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 1.8, w: 4.3, h: 2.5,
  fill: { color: colors.lightBg }
});

const autoDriveTrends = [
  "L2+辅助驾驶普及",
  "城市NOA逐步开放",
  "Robotaxi商业化试点",
  "软件定义汽车",
  "AI大模型上车"
];

autoDriveTrends.forEach((trend, index) => {
  slide9.addText("→ " + trend, {
    x: 0.7, y: 1.9 + index * 0.45, w: 4.0, h: 0.4,
    fontSize: 12, color: colors.text,
    fontFace: "Microsoft YaHei"
  });
});

// 能源互联网
slide9.addText("能源互联网", {
  x: 5.0, y: 1.3, w: 4.5, h: 0.5,
  fontSize: 18, bold: true, color: colors.highlight,
  fontFace: "Microsoft YaHei"
});

slide9.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 5.0, y: 1.8, w: 4.5, h: 2.5,
  fill: { color: colors.lightBg }
});

const energyTrends = [
  "V2G车辆到电网",
  "虚拟电厂聚合",
  "光储充一体化",
  "绿电交易",
  "智慧能源管理"
];

energyTrends.forEach((trend, index) => {
  slide9.addText("→ " + trend, {
    x: 5.2, y: 1.9 + index * 0.45, w: 4.2, h: 0.4,
    fontSize: 12, color: colors.text,
    fontFace: "Microsoft YaHei"
  });
});

// 融合趋势
slide9.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 4.5, w: 9, h: 1.0,
  fill: { color: colors.primary }
});

slide9.addText("电动化 + 智能化 + 网联化 + 共享化 = 未来出行新生态", {
  x: 0.5, y: 4.5, w: 9, h: 1.0,
  fontSize: 16, bold: true, color: colors.white,
  fontFace: "Microsoft YaHei", align: "center", valign: "middle"
});

// ========== 第10页：结论 ==========
const slide10 = pptx.addSlide();
slide10.background = { color: colors.white };

slide10.addText("结论", {
  x: 0.5, y: 0.4, w: 9, h: 0.7,
  fontSize: 32, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide10.addShape(pptx.shapes.RECTANGLE, {
  x: 0.5, y: 1.0, w: 1.5, h: 0.04,
  fill: { color: colors.secondary }
});

// 核心结论
slide10.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 1.3, w: 9, h: 3.0,
  fill: { color: colors.lightBg }
});

const conclusions = [
  "新能源电动车已成为不可逆转的全球趋势，技术、市场、政策三重驱动",
  "中国在全球电动化转型中处于领先地位，形成了完整产业链",
  "电池技术仍是关键瓶颈，固态电池等新技术有望突破",
  "智能化将重新定义汽车，从交通工具转变为智能终端",
  "基础设施和成本问题需要政府与企业协同解决",
  "未来将呈现电动化、智能化、网联化、共享化融合发展"
];

conclusions.forEach((conclusion, index) => {
  slide10.addText((index + 1) + ". " + conclusion, {
    x: 0.7, y: 1.4 + index * 0.5, w: 8.6, h: 0.45,
    fontSize: 12, color: colors.text,
    fontFace: "Microsoft YaHei"
  });
});

// 展望
slide10.addText("展望", {
  x: 0.5, y: 4.5, w: 9, h: 0.5,
  fontSize: 18, bold: true, color: colors.primary,
  fontFace: "Microsoft YaHei"
});

slide10.addShape(pptx.shapes.ROUNDED_RECTANGLE, {
  x: 0.5, y: 5.0, w: 9, h: 0.7,
  fill: { color: colors.secondary }
});

slide10.addText("新能源电动车不仅是汽车产业的变革，更是能源革命、交通革命和数字革命的交汇点", {
  x: 0.5, y: 5.0, w: 9, h: 0.7,
  fontSize: 14, bold: true, color: colors.white,
  fontFace: "Microsoft YaHei", align: "center", valign: "middle"
});

// ========== 第11页：结束页 ==========
const slide11 = pptx.addSlide();
slide11.background = { color: colors.primary };

slide11.addText("谢谢", {
  x: 0, y: 1.8, w: 10, h: 1.0,
  fontSize: 56, bold: true, color: colors.white,
  fontFace: "Microsoft YaHei", align: "center"
});

slide11.addShape(pptx.shapes.RECTANGLE, {
  x: 4.0, y: 2.9, w: 2, h: 0.04,
  fill: { color: colors.highlight }
});

slide11.addText("迈向可持续、智能、高效的未来出行", {
  x: 0, y: 3.2, w: 10, h: 0.5,
  fontSize: 18, color: colors.highlight,
  fontFace: "Microsoft YaHei", align: "center"
});

slide11.addText("新能源电动车发展 - 技术演进、市场现状与未来趋势", {
  x: 0, y: 4.0, w: 10, h: 0.5,
  fontSize: 14, color: colors.lightText,
  fontFace: "Microsoft YaHei", align: "center"
});

// 保存 PPT
pptx.writeFile({ fileName: "新能源电动车发展.pptx" })
  .then(() => {
    console.log("PPT 已成功生成：新能源电动车发展.pptx");
  })
  .catch(err => {
    console.error("生成 PPT 时出错：", err);
  });