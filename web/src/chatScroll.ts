// 回答在往外写的时候，画面跟不跟着往下走。
//
// 从前 `shown` 每变一次就把底部的哨兵滚进视野，而生成期间 liveAnswer 每 33ms 通知一次：
// 往上翻着读前文（轨迹、上一轮回答、引用）的人，每 33ms 被拽回底部一次。一个正在写的
// 回答让屏幕上别的东西都读不成。
//
// 判据改成**人此刻停在哪**：停在底部就跟着新写出的字走；翻上去了就留在原地，自己滚回
// 底部再接着跟。「底部」留一点余量——列表底下有 pb-12 的留白，换行与缩放也会让最后
// 几像素对不齐。要求分毫不差的话，一个一直停在底部的人会被当成「翻上去了」而跟丢。

/** 离底部多近算「在跟着读」，CSS 像素 */
export const FOLLOW_SLACK_PX = 48;

/** 这个滚动容器此刻是不是停在底部。内容比视口短时恒为真：没有地方可以翻走 */
export function followsBottom(el: {
  scrollTop: number;
  clientHeight: number;
  scrollHeight: number;
}): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_SLACK_PX;
}
