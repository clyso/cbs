/*
 * Copyright © 2026 Clyso GmbH
 *
 *  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  https://www.gnu.org/licenses/agpl-3.0.html
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */

import { describe, expect, it } from 'vitest';
import { BuildStateHelper } from './buildStateHelper';
import {
  ErrorBuildState,
  NeutralBuildState,
  SuccessBuildState,
  WarningBuildState,
} from '@/utils/types/cbs';

describe('BuildStateHelper', () => {
  describe('toSentenceCase', () => {
    it('lowercases everything except the first character', () => {
      expect(BuildStateHelper.toSentenceCase(NeutralBuildState.STARTED)).toBe(
        'Started',
      );
      expect(BuildStateHelper.toSentenceCase(ErrorBuildState.FAILURE)).toBe(
        'Failure',
      );
      expect(BuildStateHelper.toSentenceCase(SuccessBuildState.SUCCESS)).toBe(
        'Success',
      );
    });
  });

  describe('isNeutralBuildState', () => {
    it('returns true for all neutral states', () => {
      expect(BuildStateHelper.isNeutralBuildState(NeutralBuildState.NEW)).toBe(
        true,
      );
      expect(
        BuildStateHelper.isNeutralBuildState(NeutralBuildState.PENDING),
      ).toBe(true);
      expect(
        BuildStateHelper.isNeutralBuildState(NeutralBuildState.STARTED),
      ).toBe(true);
      expect(
        BuildStateHelper.isNeutralBuildState(NeutralBuildState.RETRY),
      ).toBe(true);
    });

    it('returns false for non-neutral states', () => {
      expect(
        BuildStateHelper.isNeutralBuildState(SuccessBuildState.SUCCESS),
      ).toBe(false);
      expect(
        BuildStateHelper.isNeutralBuildState(ErrorBuildState.FAILURE),
      ).toBe(false);
      expect(
        BuildStateHelper.isNeutralBuildState(WarningBuildState.REVOKED),
      ).toBe(false);
    });
  });

  describe('isSuccessBuildState', () => {
    it('returns true for SUCCESS', () => {
      expect(
        BuildStateHelper.isSuccessBuildState(SuccessBuildState.SUCCESS),
      ).toBe(true);
    });

    it('returns false for non-success states', () => {
      expect(
        BuildStateHelper.isSuccessBuildState(NeutralBuildState.STARTED),
      ).toBe(false);
      expect(
        BuildStateHelper.isSuccessBuildState(ErrorBuildState.REJECTED),
      ).toBe(false);
    });
  });

  describe('isWarningBuildState', () => {
    it('returns true for REVOKED', () => {
      expect(
        BuildStateHelper.isWarningBuildState(WarningBuildState.REVOKED),
      ).toBe(true);
    });

    it('returns false for non-warning states', () => {
      expect(
        BuildStateHelper.isWarningBuildState(SuccessBuildState.SUCCESS),
      ).toBe(false);
      expect(
        BuildStateHelper.isWarningBuildState(ErrorBuildState.FAILURE),
      ).toBe(false);
    });
  });

  describe('isErrorBuildState', () => {
    it('returns true for all error states', () => {
      expect(BuildStateHelper.isErrorBuildState(ErrorBuildState.FAILURE)).toBe(
        true,
      );
      expect(BuildStateHelper.isErrorBuildState(ErrorBuildState.REJECTED)).toBe(
        true,
      );
    });

    it('returns false for non-error states', () => {
      expect(BuildStateHelper.isErrorBuildState(NeutralBuildState.NEW)).toBe(
        false,
      );
    });
  });
});
